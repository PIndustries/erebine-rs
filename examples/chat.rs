use erebine::{
    operations,
    types::{ChatCompletionRequest, Message},
    Client, Routing, Value,
};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::router(
        &std::env::var("EREBINE_API_KEY")?,
        &std::env::var("EREBINE_PROJECT_ID")?,
        Routing::Direct {
            workspace: Some(std::env::var("EREBINE_WORKSPACE_ID")?),
        },
    )?;
    // Discover the model ID on this exact base, then pass it verbatim.
    let models: Value = client
        .operation(operations::MODELS_LIST)
        .send()
        .await?
        .json()?;
    println!("Models: {models}");
    let request = ChatCompletionRequest::new(
        std::env::var("EREBINE_MODEL")?,
        vec![Message::user("Say hello in one sentence.")],
    );
    let response = client
        .operation(operations::CHAT_COMPLETIONS_CREATE)
        .json(&request)?
        .send()
        .await?;
    println!("{}", response.text()?);
    Ok(())
}
