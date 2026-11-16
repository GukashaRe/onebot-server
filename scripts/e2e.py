#!/usr/bin/env python3
"""End-to-end acceptance probe for the OneBot 11 prototype.

Requires the server to be running and the Python `websockets` package.
Environment:
  BASE_URL         default http://127.0.0.1:8080
  REGISTER_KEY     required; must match the server
  ADMIN_KEY        required; must match the server
  RUN_SLOW_TEST    default 1; set 0 to skip the backpressure test
  E2E_STATE_FILE   optional path to write test fixture tokens/group for restart checks
"""

from __future__ import annotations

import asyncio
import base64
import json
import os
import secrets
import socket
import sys
import time
import urllib.error
import urllib.request

import websockets

BASE = os.environ.get("BASE_URL", "http://127.0.0.1:8080").rstrip("/")
REGISTER_KEY = os.environ.get("REGISTER_KEY", "")
ADMIN_KEY = os.environ.get("ADMIN_KEY", "")
RUN_SLOW_TEST = os.environ.get("RUN_SLOW_TEST", "1") == "1"
SUFFIX = (str(int(time.time()))[-4:] + secrets.token_hex(2))
PASSED: list[str] = []

if not REGISTER_KEY or not ADMIN_KEY:
    print("REGISTER_KEY and ADMIN_KEY must be set in the environment", file=sys.stderr)
    raise SystemExit(2)


def check(condition: bool, label: str) -> None:
    if not condition:
        raise AssertionError(label)
    PASSED.append(label)
    print(f"  PASS  {label}")


def http(
    method: str,
    path: str,
    body: dict | None = None,
    token: str | None = None,
    headers: dict | None = None,
) -> tuple[int, dict]:
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(BASE + path, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    if token:
        req.add_header("Authorization", "Bearer " + token)
    for key, value in (headers or {}).items():
        req.add_header(key, value)
    try:
        with urllib.request.urlopen(req, timeout=30) as response:
            return response.status, json.loads(response.read())
    except urllib.error.HTTPError as exc:
        raw = exc.read()
        try:
            return exc.code, json.loads(raw)
        except Exception:
            return exc.code, {"raw": raw.decode(errors="replace")}


def register(username: str, with_key: bool = True) -> dict:
    body = {"username": username, "labels": ["e2e"]}
    if with_key:
        body["register_key"] = REGISTER_KEY
    status, payload = http("POST", "/register", body)
    if status != 200:
        raise AssertionError(f"register {username} failed: {status} {payload}")
    return payload


def action(token: str, name: str, params: dict) -> tuple[int, dict]:
    return http("POST", "/", {"action": name, "params": params}, token)


def assert_action_ok(token: str, name: str, params: dict) -> dict:
    status, payload = action(token, name, params)
    check(status == 200 and payload.get("status") == "ok" and payload.get("retcode") == 0,
          f"action {name} succeeds through HTTP")
    return payload["data"]


def expect_action_retcode(token: str, name: str, params: dict, retcode: int, label: str) -> dict:
    status, payload = action(token, name, params)
    check(status == 200 and payload.get("status") == "failed" and payload.get("retcode") == retcode,
          label)
    return payload


def notice_event(notice_type: str, sub_type: str | None = None,
                 user_id: int | None = None, group_id: int | None = None):
    def predicate(event: dict) -> bool:
        if event.get("post_type") != "notice" or event.get("notice_type") != notice_type:
            return False
        if sub_type is not None and event.get("sub_type") != sub_type:
            return False
        if user_id is not None and event.get("user_id") != user_id:
            return False
        if group_id is not None and event.get("group_id") != group_id:
            return False
        return True
    return predicate


class WSClient:
    def __init__(self, name: str, ws) -> None:
        self.name = name
        self.ws = ws
        self.events: list[dict] = []
        self.cond = asyncio.Condition()
        self.closed = asyncio.Event()
        self.reader = asyncio.create_task(self._read_loop())

    @classmethod
    async def connect(cls, name: str, token: str) -> "WSClient":
        ws = await websockets.connect(
            BASE.replace("http://", "ws://").replace("https://", "wss://")
            + "/ws?access_token="
            + token,
            open_timeout=10,
            close_timeout=3,
        )
        return cls(name, ws)

    async def _read_loop(self) -> None:
        try:
            async for raw in self.ws:
                if isinstance(raw, bytes):
                    continue
                event = json.loads(raw)
                async with self.cond:
                    self.events.append(event)
                    self.cond.notify_all()
        except Exception:
            pass
        finally:
            self.closed.set()
            async with self.cond:
                self.cond.notify_all()

    async def wait(self, predicate, timeout: float = 10) -> dict:
        deadline = asyncio.get_running_loop().time() + timeout

        async def inner():
            async with self.cond:
                while True:
                    for index, event in enumerate(self.events):
                        if predicate(event):
                            return self.events.pop(index)
                    remaining = deadline - asyncio.get_running_loop().time()
                    if remaining <= 0:
                        raise TimeoutError(
                            f"{self.name}: timed out; buffered={self.events[-5:]}"
                        )
                    try:
                        await asyncio.wait_for(self.cond.wait(), remaining)
                    except TimeoutError:
                        raise TimeoutError(
                            f"{self.name}: timed out; buffered={self.events[-5:]}"
                        ) from None

        return await inner()

    async def wait_closed(self, timeout: float = 10) -> None:
        await asyncio.wait_for(self.closed.wait(), timeout)

    async def close(self) -> None:
        if not self.closed.is_set():
            await self.ws.close()
        try:
            await asyncio.wait_for(self.reader, 5)
        except Exception:
            self.reader.cancel()


def msg_event(message_id: int | None = None, message_type: str | None = None, text: str | None = None):
    def predicate(event: dict) -> bool:
        if event.get("post_type") != "message":
            return False
        if message_id is not None and event.get("message_id") != message_id:
            return False
        if message_type is not None and event.get("message_type") != message_type:
            return False
        if text is not None:
            segments = event.get("message") or []
            joined = "".join(
                s.get("data", {}).get("text", "") for s in segments if s.get("type") == "text"
            )
            if joined != text:
                return False
        return True

    return predicate


def text_of(event: dict) -> str:
    return "".join(
        s.get("data", {}).get("text", "") for s in event.get("message", []) if s.get("type") == "text"
    )


async def slow_ws_handshake(token: str) -> socket.socket:
    """Complete a WS upgrade and then deliberately never read again."""
    sock = socket.create_connection(("127.0.0.1", int(BASE.rsplit(":", 1)[1])), timeout=5)
    key = base64.b64encode(os.urandom(16)).decode()
    request = (
        f"GET /ws?access_token={token} HTTP/1.1\r\n"
        "Host: 127.0.0.1:8080\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        f"Sec-WebSocket-Key: {key}\r\n"
        "Sec-WebSocket-Version: 13\r\n\r\n"
    )
    sock.sendall(request.encode())
    received = b""
    while b"\r\n\r\n" not in received:
        chunk = sock.recv(4096)
        if not chunk:
            raise AssertionError("slow WS handshake closed early")
        received += chunk
    if b"101 Switching Protocols" not in received.split(b"\r\n", 1)[0]:
        raise AssertionError(f"slow WS handshake failed: {received[:200]!r}")
    return sock


async def main() -> None:
    print(f"target={BASE} suffix={SUFFIX}")

    print("[1] registration, 409, token hash storage, auth")
    alice = register(f"alice_{SUFFIX}")
    bob = register(f"bob_{SUFFIX}")
    carol = register(f"carol_{SUFFIX}")
    check(bool(alice["token"]) and len(alice["token"]) >= 40, "registration returns a token")
    status, _ = http(
        "POST",
        "/register",
        {"username": f"ALICE_{SUFFIX}", "register_key": REGISTER_KEY},
    )
    check(status == 409, "case-insensitive duplicate username returns 409")
    status, _ = http("POST", "/register", {"username": f"nokey_{SUFFIX}"})
    check(status == 403, "registration without register_key returns 403")
    status, payload = action(alice["token"], "get_login_info", {})
    check(status == 200 and payload["data"]["user_id"] == alice["account_id"],
          "HTTP action authenticates bearer token")

    a_ws = await WSClient.connect("alice", alice["token"])
    b_ws = await WSClient.connect("bob", bob["token"])
    c_ws = await WSClient.connect("carol", carol["token"])
    check(True, "three accounts connect to WS with query token")

    print("[2] private message loop and sender echo")
    private_text = f"private-{SUFFIX}"
    data = assert_action_ok(
        alice["token"],
        "send_private_msg",
        {"user_id": bob["account_id"], "message": [{"type": "text", "data": {"text": private_text}}]},
    )
    check(isinstance(data.get("message_id"), int), "send_private_msg returns numeric message_id")
    mid = data["message_id"]
    a_priv = await a_ws.wait(msg_event(message_id=mid))
    b_priv = await b_ws.wait(msg_event(message_id=mid))
    check(a_priv["self_id"] == alice["account_id"] and b_priv["self_id"] == bob["account_id"],
          "private event self_id is filled per receiver")
    check(a_priv["message_type"] == "private" and text_of(a_priv) == private_text,
          "private WS event is an OB11 message event")
    check(b_priv["message_type"] == "private" and text_of(b_priv) == private_text,
          "private receiver gets the OB11 message event")
    check(a_priv["message_id"] == b_priv["message_id"], "sender echo carries the same message_id")
    check(a_priv.get("peer_id") == bob["account_id"] and b_priv.get("peer_id") == alice["account_id"],
          "private events expose a peer_id extension for client routing")

    print("[3] friend request/approve loop")
    status, friend_req = http(
        "POST", "/friends/request", {"target_id": bob["account_id"]}, alice["token"]
    )
    check(status == 200 and friend_req["status"] == "pending", "friend request REST succeeds")
    request_event = await b_ws.wait(
        lambda e: e.get("post_type") == "request" and e.get("request_type") == "friend"
    )
    check(request_event["self_id"] == bob["account_id"], "friend request self_id is target")
    check(request_event["user_id"] == alice["account_id"], "friend request user_id is applicant")
    check(str(request_event["flag"]) == str(friend_req["request_id"]), "friend request flag is request id")
    status, request_inbox = http("GET", "/requests", token=bob["token"])
    check(status == 200 and any(
        item["from_id"] == alice["account_id"] and item["from_username"] == alice["username"]
        for item in request_inbox.get("friend_requests", [])
    ), "GET /requests lists the pending friend request with sender name")
    status, approved = http(
        "POST",
        "/friends/approve",
        {"flag": request_event["flag"], "approve": True},
        bob["token"],
    )
    check(status == 200 and approved["status"] == "approved", "friend approve accepts flag")
    a_friends = assert_action_ok(alice["token"], "get_friend_list", {})
    b_friends = assert_action_ok(bob["token"], "get_friend_list", {})
    check(any(f["user_id"] == bob["account_id"] for f in a_friends), "alice friend list contains bob")
    check(any(f["user_id"] == alice["account_id"] for f in b_friends), "bob friend list contains alice")

    print("[4] group create/join/approve loop and notices")
    status, group = http("POST", "/groups", {"name": f"Group {SUFFIX}"}, alice["token"])
    check(status == 200 and group["owner_id"] == alice["account_id"], "POST /groups makes creator owner")
    group_id = group["group_id"]
    status, join_b = http("POST", f"/groups/{group_id}/join", {}, bob["token"])
    check(status == 200, "bob can request to join group")
    join_b_event = await a_ws.wait(
        lambda e: e.get("post_type") == "request"
        and e.get("request_type") == "group"
        and e.get("user_id") == bob["account_id"]
    )
    check(join_b_event.get("sub_type") == "add", "group request sub_type is add")
    check(join_b_event.get("group_id") == group_id, "group request group_id matches")
    status, join_c = http("POST", f"/groups/{group_id}/join", {}, carol["token"])
    check(status == 200, "carol can request to join group")
    join_c_event = await a_ws.wait(
        lambda e: e.get("post_type") == "request"
        and e.get("request_type") == "group"
        and e.get("user_id") == carol["account_id"]
    )
    status, owner_inbox = http("GET", "/requests", token=alice["token"])
    check(status == 200 and any(
        item["request_id"] == join_c["request_id"] and item["account_username"] == carol["username"]
        for item in owner_inbox.get("group_join_requests", [])
    ), "GET /requests lists actionable group join requests for the owner")
    status, _ = http(
        "POST",
        f"/groups/{group_id}/members/approve",
        {"request_id": join_b["request_id"], "approve": True},
        alice["token"],
    )
    check(status == 200, "owner approves bob using request_id")
    inc_b = await a_ws.wait(
        lambda e: e.get("notice_type") == "group_increase" and e.get("user_id") == bob["account_id"]
    )
    check(inc_b["group_id"] == group_id and inc_b["operator_id"] == alice["account_id"],
          "group_increase notice has group and operator")
    await b_ws.wait(
        lambda e: e.get("notice_type") == "group_increase" and e.get("user_id") == bob["account_id"]
    )
    status, _ = http(
        "POST",
        f"/groups/{group_id}/members/approve",
        {"flag": join_c_event["flag"], "approve": True},
        alice["token"],
    )
    check(status == 200, "owner approves carol using flag")
    inc_c = await c_ws.wait(
        lambda e: e.get("notice_type") == "group_increase" and e.get("user_id") == carol["account_id"]
    )
    check(inc_c["group_id"] == group_id, "new member receives group_increase notice")
    a_groups = assert_action_ok(alice["token"], "get_group_list", {})
    check(any(g["group_id"] == group_id and g.get("role") == "owner" for g in a_groups),
          "get_group_list returns group for owner")
    members = assert_action_ok(alice["token"], "get_group_member_list", {"group_id": group_id})
    check({m["user_id"] for m in members} == {alice["account_id"], bob["account_id"], carol["account_id"]},
          "get_group_member_list contains all approved members")

    print("[5] group message fanout")
    group_text = f"group-{SUFFIX}"
    data = assert_action_ok(
        alice["token"],
        "send_group_msg",
        {"group_id": group_id, "message": [{"type": "text", "data": {"text": group_text}}]},
    )
    group_mid = data["message_id"]
    a_group = await a_ws.wait(msg_event(message_id=group_mid))
    b_group = await b_ws.wait(msg_event(message_id=group_mid))
    c_group = await c_ws.wait(msg_event(message_id=group_mid))
    check(a_group["self_id"] == alice["account_id"]
          and b_group["self_id"] == bob["account_id"]
          and c_group["self_id"] == carol["account_id"],
          "group message self_id is filled per receiver")
    check(all(e["message_type"] == "group" and e["group_id"] == group_id
              for e in (a_group, b_group, c_group)),
          "group message reaches all members with group_id")
    check(len({a_group["message_id"], b_group["message_id"], c_group["message_id"]}) == 1,
          "group message uses one global message_id")

    send_msg_private_text = f"sendmsg-private-{SUFFIX}"
    send_msg_private = assert_action_ok(
        alice["token"],
        "send_msg",
        {
            "message_type": "private",
            "user_id": bob["account_id"],
            "message": [{"type": "text", "data": {"text": send_msg_private_text}}],
        },
    )
    await a_ws.wait(msg_event(message_id=send_msg_private["message_id"]))
    await b_ws.wait(msg_event(message_id=send_msg_private["message_id"]))
    check(True, "send_msg dispatches private messages")

    send_msg_group_text = f"sendmsg-group-{SUFFIX}"
    send_msg_group = assert_action_ok(
        alice["token"],
        "send_msg",
        {
            "message_type": "group",
            "group_id": group_id,
            "message": [{"type": "text", "data": {"text": send_msg_group_text}}],
        },
    )
    await b_ws.wait(msg_event(message_id=send_msg_group["message_id"]))
    await c_ws.wait(msg_event(message_id=send_msg_group["message_id"]))
    check(True, "send_msg dispatches group messages")

    bob_member_info = assert_action_ok(
        alice["token"],
        "get_group_member_info",
        {"group_id": group_id, "user_id": bob["account_id"]},
    )
    check(bob_member_info["role"] == "member" and bob_member_info["group_id"] == group_id,
          "get_group_member_info returns neutral group membership data")

    print("[6] history cursor, pagination, offline catch-up")
    history = assert_action_ok(
        alice["token"], "get_group_msg_history", {"group_id": group_id, "limit": 100}
    )
    messages = history["messages"]
    check(any(m["message_id"] == group_mid for m in messages), "group history contains group message")
    collected = []
    after = 0
    while True:
        page = assert_action_ok(
            alice["token"],
            "get_group_msg_history",
            {"group_id": group_id, "after_seq": after, "limit": 2},
        )["messages"]
        if not page:
            break
        collected.extend(page)
        after = page[-1]["message_seq"]
        if len(collected) > 100:
            raise AssertionError("pagination did not terminate")
    ids = [m["message_id"] for m in collected]
    check(len(ids) == len(set(ids)), "message_seq pagination has no duplicate message_id")
    check([m["message_seq"] for m in collected] == sorted(m["message_seq"] for m in collected),
          "message_seq pagination is ascending")
    check(ids == [m["message_id"] for m in messages],
          "message_seq pagination matches full history without omissions")
    newest_seq = max(m["message_seq"] for m in messages)
    await b_ws.close()
    offline_group_text = f"offline-{SUFFIX}"
    offline_mid = assert_action_ok(
        alice["token"],
        "send_group_msg",
        {"group_id": group_id, "message": [{"type": "text", "data": {"text": offline_group_text}}]},
    )["message_id"]
    caught = assert_action_ok(
        alice["token"],
        "get_group_msg_history",
        {"group_id": group_id, "after_seq": newest_seq, "limit": 100},
    )["messages"]
    check(any(m["message_id"] == offline_mid for m in caught),
          "after_seq catches messages sent while a member was offline")
    check(newest_seq not in [m["message_seq"] for m in caught],
          "after_seq excludes already-seen sequence")
    b_ws = await WSClient.connect("bob", bob["token"])
    b_caught = assert_action_ok(
        bob["token"], "get_group_msg_history", {"group_id": group_id, "after_seq": newest_seq}
    )["messages"]
    check([m["message_id"] for m in b_caught] == [offline_mid],
          "reconnected member catches exactly the missed message")
    private_history = assert_action_ok(
        bob["token"], "get_friend_msg_history", {"user_id": alice["account_id"], "limit": 100}
    )["messages"]
    check(any(m["message_id"] == mid for m in private_history), "private history is available to both parties")
    await b_ws.close()
    last_private_seq = max(m["message_seq"] for m in private_history)
    offline_private_mid = assert_action_ok(
        alice["token"],
        "send_private_msg",
        {"user_id": bob["account_id"], "message": [{"type": "text", "data": {"text": "offline-private"}}]},
    )["message_id"]
    b_ws = await WSClient.connect("bob", bob["token"])
    private_caught = assert_action_ok(
        bob["token"],
        "get_friend_msg_history",
        {"user_id": alice["account_id"], "after_seq": last_private_seq},
    )["messages"]
    check([m["message_id"] for m in private_caught] == [offline_private_mid],
          "private history catch-up is exact by message_seq")

    print("[7] unsupported action follows OB11 error shape")
    status, unsupported = action(alice["token"], "set_group_leave", {"group_id": group_id})
    check(status == 200, "unsupported action is HTTP 200, not 500")
    check(unsupported.get("status") == "failed" and unsupported.get("retcode") == 1404,
          "unsupported action uses retcode 1404")
    check(set(unsupported.keys()) == {"status", "retcode", "data"} and unsupported["data"] is None,
          "unsupported action response is the standard OB11 envelope")

    print("[8] token reset revokes HTTP and kicks WS")
    dave = register(f"dave_{SUFFIX}")
    d_ws = await WSClient.connect("dave", dave["token"])
    status, reset = http("POST", "/me/token/reset", {}, dave["token"])
    check(status == 200 and reset["token"] != dave["token"], "self token reset returns a new token")
    await d_ws.wait_closed(8)
    check(True, "old WS is kicked on token reset")
    status, _ = action(dave["token"], "get_login_info", {})
    check(status == 401, "old token gets HTTP 401 after reset")
    status, payload = action(reset["token"], "get_login_info", {})
    check(status == 200 and payload["status"] == "ok", "new token works immediately")
    status, admin_reset = http(
        "POST",
        f"/admin/accounts/{dave['account_id']}/token/reset",
        {},
        headers={"X-Admin-Key": ADMIN_KEY, "X-Admin-Actor": "e2e-operator"},
    )
    check(status == 200 and admin_reset["token"] != reset["token"], "admin token reset works with ADMIN_KEY")
    status, _ = action(reset["token"], "get_login_info", {})
    check(status == 401, "admin-reset invalidates previous token")

    if RUN_SLOW_TEST:
        print("[9] slow WS does not block a fast WS (bounded queue)")
        await a_ws.close()
        await b_ws.close()
        await c_ws.close()
        fast = await WSClient.connect("carol-fast", carol["token"])
        slow_sock = await slow_ws_handshake(bob["token"])
        await asyncio.sleep(0.3)
        payload = "x" * 16384
        total = 400
        semaphore = asyncio.Semaphore(20)
        failures: list[str] = []

        async def send_one(index: int) -> None:
            async with semaphore:
                status, response = await asyncio.to_thread(
                    action,
                    alice["token"],
                    "send_group_msg",
                    {
                        "group_id": group_id,
                        "message": [{"type": "text", "data": {"text": f"{index}:{payload}"}}],
                    },
                )
                if status != 200 or response.get("status") != "ok":
                    failures.append(f"{status}:{response}")

        started = time.monotonic()
        await asyncio.gather(*(send_one(i) for i in range(total)))
        elapsed = time.monotonic() - started
        check(not failures, f"{total} large group sends succeeded under backpressure")
        received = 0
        deadline = time.monotonic() + 45
        while received < total and time.monotonic() < deadline:
            try:
                await fast.wait(
                    lambda e: e.get("post_type") == "message" and e.get("message_type") == "group",
                    timeout=max(1, deadline - time.monotonic()),
                )
                received += 1
            except TimeoutError:
                break
        check(received == total, f"fast WS received all {total} messages while slow WS was stalled")
        await asyncio.sleep(0.5)
        _, health = http("GET", "/health")
        check(health.get("slow_disconnects", 0) >= 1, "server recorded at least one slow-consumer disconnect")
        check(elapsed < 60, "fanout completed without waiting on the slow connection")
        slow_sock.close()
        await fast.close()

    print("[10] group leave and permission checks")
    for client in (a_ws, b_ws, c_ws):
        if not client.closed.is_set():
            await client.close()
    a_ws = await WSClient.connect("alice-leave", alice["token"])
    b_ws = await WSClient.connect("bob-leave", bob["token"])
    c_ws = await WSClient.connect("carol-leave", carol["token"])
    status, owner_leave = http("POST", f"/groups/{group_id}/leave", {}, alice["token"])
    check(status == 403 and owner_leave.get("error", {}).get("code") == "forbidden",
          "owner cannot leave the group")
    status, _ = http("POST", f"/groups/{group_id}/leave", {}, carol["token"])
    check(status == 200, "non-owner member can leave the group")
    decrease_a = await a_ws.wait(
        lambda e: e.get("notice_type") == "group_decrease" and e.get("user_id") == carol["account_id"]
    )
    await b_ws.wait(
        lambda e: e.get("notice_type") == "group_decrease" and e.get("user_id") == carol["account_id"]
    )
    check(decrease_a["group_id"] == group_id and decrease_a["operator_id"] == carol["account_id"],
          "group_decrease notice reaches remaining members")
    try:
        await c_ws.wait(lambda e: e.get("notice_type") == "group_decrease", timeout=1.0)
        leaver_got_notice = True
    except TimeoutError:
        leaver_got_notice = False
    check(not leaver_got_notice, "leaver does not receive their own group_decrease notice")
    carol_groups = assert_action_ok(carol["token"], "get_group_list", {})
    check(not any(g["group_id"] == group_id for g in carol_groups),
          "left member no longer sees the group")
    status, denied_history = action(
        carol["token"], "get_group_msg_history", {"group_id": group_id, "limit": 10}
    )
    check(status == 200 and denied_history.get("status") == "failed"
          and denied_history.get("retcode") == 1403,
          "non-member group history request is denied with OB11 retcode 1403")
    remaining = assert_action_ok(alice["token"], "get_group_member_list", {"group_id": group_id})
    check(carol["account_id"] not in {m["user_id"] for m in remaining},
          "group member list reflects the departure")
    await a_ws.close()
    await b_ws.close()
    await c_ws.close()

    print("[11] group management actions, permissions, notices, bans")
    owner2 = register(f"owner2_{SUFFIX}")
    admin2 = register(f"admin2_{SUFFIX}")
    m1 = register(f"m1_{SUFFIX}")
    m2 = register(f"m2_{SUFFIX}")
    m3 = register(f"m3_{SUFFIX}")
    m4 = register(f"m4_{SUFFIX}")
    m5 = register(f"m5_{SUFFIX}")
    status, group2 = http("POST", "/groups", {"name": f"Mgmt {SUFFIX}"}, owner2["token"])
    check(status == 200, "group management fixture group created")
    gid = group2["group_id"]
    for member in (admin2, m1, m2, m3, m4, m5):
        status, join_payload = http("POST", f"/groups/{gid}/join", {}, member["token"])
        check(status == 200, f"{member['username']} joins management group")
        status, _ = http(
            "POST",
            f"/groups/{gid}/members/approve",
            {"request_id": join_payload["request_id"], "approve": True},
            owner2["token"],
        )
        check(status == 200, f"{member['username']} is approved into management group")

    mgmt_clients = {
        name: await WSClient.connect(name, account["token"])
        for name, account in [
            ("owner2", owner2),
            ("admin2", admin2),
            ("m1", m1),
            ("m2", m2),
            ("m3", m3),
            ("m4", m4),
            ("m5", m5),
        ]
    }

    # ---- set_group_admin set / admin kick member / kick_me / removed fanout
    assert_action_ok(owner2["token"], "set_group_admin",
                     {"group_id": gid, "user_id": admin2["account_id"], "enable": True})
    set_admin_notice = await mgmt_clients["admin2"].wait(
        notice_event("group_admin", "set", admin2["account_id"], gid)
    )
    check(set_admin_notice["self_id"] == admin2["account_id"]
          and set_admin_notice["operator_id"] == owner2["account_id"],
          "group_admin set notice has per-account self_id and correct operator")
    await mgmt_clients["m2"].wait(notice_event("group_admin", "set", admin2["account_id"], gid))
    check(True, "group_admin set notice reaches other members")

    expect_action_retcode(owner2["token"], "set_group_kick",
                          {"group_id": gid, "user_id": owner2["account_id"]},
                          1403, "owner cannot kick themselves")

    assert_action_ok(owner2["token"], "set_group_kick",
                     {"group_id": gid, "user_id": m4["account_id"], "reject_add_request": True})
    m4_kick_me = await mgmt_clients["m4"].wait(
        notice_event("group_decrease", "kick_me", m4["account_id"], gid)
    )
    check(m4_kick_me["self_id"] == m4["account_id"]
          and m4_kick_me["operator_id"] == owner2["account_id"],
          "owner kick: target receives kick_me with own self_id")
    kick_notice_m2 = await mgmt_clients["m2"].wait(
        notice_event("group_decrease", "kick", m4["account_id"], gid)
    )
    check(kick_notice_m2["self_id"] == m2["account_id"],
          "owner kick: remaining member receives group_decrease(kick) with own self_id")

    assert_action_ok(admin2["token"], "set_group_kick",
                     {"group_id": gid, "user_id": m1["account_id"]})
    m1_kick_me = await mgmt_clients["m1"].wait(
        notice_event("group_decrease", "kick_me", m1["account_id"], gid)
    )
    check(m1_kick_me["self_id"] == m1["account_id"],
          "admin kick: kicked member receives kick_me")
    await mgmt_clients["m2"].wait(
        notice_event("group_decrease", "kick", m1["account_id"], gid)
    )
    post_kick_text = f"post-kick-{SUFFIX}"
    post_kick_mid = assert_action_ok(
        owner2["token"], "send_group_msg",
        {"group_id": gid, "message": [{"type": "text", "data": {"text": post_kick_text}}]},
    )["message_id"]
    await mgmt_clients["m2"].wait(msg_event(message_id=post_kick_mid))
    try:
        await mgmt_clients["m1"].wait(msg_event(message_id=post_kick_mid), timeout=1.0)
        kicked_received = True
    except TimeoutError:
        kicked_received = False
    check(not kicked_received, "kicked member no longer receives group messages")

    # ---- admin vs admin / admin vs owner / member attempts
    assert_action_ok(owner2["token"], "set_group_admin",
                     {"group_id": gid, "user_id": m2["account_id"], "enable": True})
    await mgmt_clients["m2"].wait(notice_event("group_admin", "set", m2["account_id"], gid))
    expect_action_retcode(admin2["token"], "set_group_admin",
                          {"group_id": gid, "user_id": m2["account_id"], "enable": True},
                          1403, "admin cannot set or unset admins")
    expect_action_retcode(admin2["token"], "set_group_kick",
                          {"group_id": gid, "user_id": m2["account_id"]},
                          1403, "admin cannot kick another admin")
    expect_action_retcode(admin2["token"], "set_group_kick",
                          {"group_id": gid, "user_id": owner2["account_id"]},
                          1403, "admin cannot kick the owner")
    expect_action_retcode(m3["token"], "set_group_kick",
                          {"group_id": gid, "user_id": m2["account_id"]},
                          1403, "member cannot kick anyone")

    # ---- set_group_admin by owner, promoted member kicks a member, then unset
    assert_action_ok(owner2["token"], "set_group_admin",
                     {"group_id": gid, "user_id": m3["account_id"], "enable": True})
    m3_admin_notice = await mgmt_clients["m3"].wait(
        notice_event("group_admin", "set", m3["account_id"], gid)
    )
    check(m3_admin_notice["self_id"] == m3["account_id"],
          "promoted member receives group_admin(set)")
    assert_action_ok(m3["token"], "set_group_kick",
                     {"group_id": gid, "user_id": m5["account_id"]})
    await mgmt_clients["m5"].wait(
        notice_event("group_decrease", "kick_me", m5["account_id"], gid)
    )
    check(True, "promoted admin can kick a regular member")
    assert_action_ok(owner2["token"], "set_group_admin",
                     {"group_id": gid, "user_id": m3["account_id"], "enable": False})
    m3_unset_notice = await mgmt_clients["m3"].wait(
        notice_event("group_admin", "unset", m3["account_id"], gid)
    )
    check(m3_unset_notice["sub_type"] == "unset",
          "demoted member receives group_admin(unset)")
    expect_action_retcode(m3["token"], "set_group_admin",
                          {"group_id": gid, "user_id": m2["account_id"], "enable": True},
                          1403, "non-owner cannot set admin")

    # ---- bans: permission matrix, message blocking, persistent row
    assert_action_ok(admin2["token"], "set_group_ban",
                     {"group_id": gid, "user_id": m3["account_id"], "duration": 60})
    ban_notice_m3 = await mgmt_clients["m3"].wait(
        notice_event("group_ban", "ban", m3["account_id"], gid)
    )
    check(ban_notice_m3["self_id"] == m3["account_id"]
          and ban_notice_m3.get("duration") == 60
          and ban_notice_m3["operator_id"] == admin2["account_id"],
          "group_ban notice includes duration/operator and target self_id")
    ban_notice_m2 = await mgmt_clients["m2"].wait(
        notice_event("group_ban", "ban", m3["account_id"], gid)
    )
    check(ban_notice_m2["self_id"] == m2["account_id"],
          "group_ban notice uses per-receiver self_id")
    history_before_ban = assert_action_ok(
        owner2["token"], "get_group_msg_history", {"group_id": gid, "limit": 100}
    )["messages"]
    expect_action_retcode(m3["token"], "send_group_msg",
                          {"group_id": gid, "message": [{"type": "text", "data": {"text": "banned"}}]},
                          1403, "banned member cannot send_group_msg")
    expect_action_retcode(m3["token"], "send_msg",
                          {"message_type": "group", "group_id": gid,
                           "message": [{"type": "text", "data": {"text": "banned"}}]},
                          1403, "banned member cannot send_msg to the group")
    history_after_ban = assert_action_ok(
        owner2["token"], "get_group_msg_history", {"group_id": gid, "limit": 100}
    )["messages"]
    check([m["message_id"] for m in history_before_ban]
          == [m["message_id"] for m in history_after_ban],
          "banned sends are rejected before writing messages")

    expect_action_retcode(admin2["token"], "set_group_ban",
                          {"group_id": gid, "user_id": owner2["account_id"], "duration": 60},
                          1403, "admin cannot ban the owner")
    expect_action_retcode(m3["token"], "set_group_ban",
                          {"group_id": gid, "user_id": m2["account_id"], "duration": 60},
                          1403, "member cannot ban anyone")
    assert_action_ok(owner2["token"], "set_group_ban",
                     {"group_id": gid, "user_id": admin2["account_id"], "duration": 30})
    await mgmt_clients["admin2"].wait(
        notice_event("group_ban", "ban", admin2["account_id"], gid)
    )
    check(True, "owner can ban an admin")
    assert_action_ok(owner2["token"], "set_group_ban",
                     {"group_id": gid, "user_id": admin2["account_id"], "duration": 0})
    unban_notice = await mgmt_clients["admin2"].wait(
        notice_event("group_ban", "lift_ban", admin2["account_id"], gid)
    )
    check(unban_notice.get("duration") == 0,
          "duration=0 produces lift_ban notice with duration 0")

    # ---- group card permissions and notices
    assert_action_ok(owner2["token"], "set_group_card",
                     {"group_id": gid, "user_id": admin2["account_id"], "card": "Boss"})
    card_notice = await mgmt_clients["admin2"].wait(
        notice_event("group_card", None, admin2["account_id"], gid)
    )
    check(card_notice["self_id"] == admin2["account_id"]
          and card_notice.get("card") == "Boss"
          and card_notice.get("old_card") == "",
          "owner card change notice has new/old card and target self_id")
    assert_action_ok(admin2["token"], "set_group_card",
                     {"group_id": gid, "user_id": m3["account_id"], "card": "Helper"})
    await mgmt_clients["m3"].wait(
        notice_event("group_card", None, m3["account_id"], gid)
    )
    check(True, "admin can change a regular member's card")
    expect_action_retcode(admin2["token"], "set_group_card",
                          {"group_id": gid, "user_id": owner2["account_id"], "card": "Nope"},
                          1403, "admin cannot change the owner's card")
    expect_action_retcode(m3["token"], "set_group_card",
                          {"group_id": gid, "user_id": m2["account_id"], "card": "Nope"},
                          1403, "member cannot change any card")
    assert_action_ok(owner2["token"], "set_group_card",
                     {"group_id": gid, "user_id": owner2["account_id"], "card": "OwnerCard"})
    await mgmt_clients["owner2"].wait(
        notice_event("group_card", None, owner2["account_id"], gid)
    )
    check(True, "owner can change their own group card")
    admin_info = assert_action_ok(owner2["token"], "get_group_member_info",
                                  {"group_id": gid, "user_id": admin2["account_id"]})
    owner_info = assert_action_ok(owner2["token"], "get_group_member_info",
                                  {"group_id": gid, "user_id": owner2["account_id"]})
    check(admin_info["card"] == "Boss" and owner_info["card"] == "OwnerCard",
          "group card changes are persisted and visible via member info")

    # ---- unsupported group-management adjacent actions remain 1404
    for unsupported_name in ("set_group_leave", "delete_msg"):
        status, payload = action(owner2["token"], unsupported_name, {})
        check(status == 200 and payload.get("status") == "failed"
              and payload.get("retcode") == 1404,
              f"{unsupported_name} returns standard unsupported error")

    state_path = os.environ.get("E2E_STATE_FILE")
    if state_path:
        with open(state_path, "w", encoding="utf-8") as handle:
            json.dump({
                "base": BASE,
                "group_id": gid,
                "m3_token": m3["token"],
                "m3_account_id": m3["account_id"],
                "owner_token": owner2["token"],
                "owner_account_id": owner2["account_id"],
            }, handle)

    for client in mgmt_clients.values():
        await client.close()

    print(f"\nALL {len(PASSED)} CHECKS PASSED")


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except Exception as exc:
        print(f"\nFAILED: {type(exc).__name__}: {exc}", file=sys.stderr)
        raise
