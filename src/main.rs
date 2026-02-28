use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    rshunterbtt::run_app().await
}
