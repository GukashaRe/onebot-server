#[tokio::main]
async fn main() -> anyhow::Result<()> {
    onebot_server::run().await
}
