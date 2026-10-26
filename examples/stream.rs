use erebine::{json, operations, Client, Routing};
use futures_util::StreamExt;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::router(
        &std::env::var("EREBINE_API_KEY")?,
        &std::env::var("EREBINE_PROJECT_ID")?,
        Routing::Endpoint(std::env::var("EREBINE_ENDPOINT")?),
    )?;
    let mut stream = client.operation(operations::CHAT_COMPLETIONS_CREATE)
        .header("x-erebine-vendor-events", "on")?
        .json(&json!({"model":std::env::var("EREBINE_MODEL")?,"messages":[{"role":"user","content":"Hello"}],"stream":true,"stream_options":{"include_usage":true}}))?
        .stream().await?;
    while let Some(event) = stream.next().await {
        println!("{:?}", event?);
    }
    Ok(())
}
