//! Reproduces the engine's exact Bedrock client construction outside the app.
//! Run: cargo run --example bedrock_probe

use aws_sdk_bedrockruntime::types::{ContentBlock, ConversationRole, Message};

#[tokio::main]
async fn main() {
    let profile = "claude-code-bedrock";
    let region = "us-east-2";
    let model =
        "us.anthropic.claude-opus-5";

    let loader = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .profile_name(profile)
        .region(aws_config::Region::new(region));
    let sdk_config = loader.load().await;
    println!("resolved region: {:?}", sdk_config.region());

    let mut conf = aws_sdk_bedrockruntime::config::Builder::from(&sdk_config);
    conf = conf.timeout_config(
        aws_sdk_bedrockruntime::config::timeout::TimeoutConfig::builder()
            .operation_timeout(std::time::Duration::from_secs(300))
            .build(),
    );
    let client = aws_sdk_bedrockruntime::Client::from_conf(conf.build());

    let msg = Message::builder()
        .role(ConversationRole::User)
        .content(ContentBlock::Text("Say OK".into()))
        .build()
        .unwrap();

    match client.converse().model_id(model).messages(msg).send().await {
        Ok(resp) => println!("SUCCESS: {:?}", resp.stop_reason()),
        Err(e) => println!(
            "FAILURE: {}",
            aws_smithy_types::error::display::DisplayErrorContext(&e)
        ),
    }
}
