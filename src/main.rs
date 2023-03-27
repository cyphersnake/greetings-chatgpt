use std::{env, sync::Arc};

use chatgpt::prelude::{ChatGPT, Conversation};
use confique::Config;
use teloxide::prelude::*;
use tracing::*;

mod bot_state;
use bot_state::{BotState as Storage, DialogueState};

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)]
    Config(#[from] confique::Error),
    #[error("Please provide config file by argument")]
    NoConfigurationFile,
    #[error(transparent)]
    Storage(#[from] bot_state::Error),
    #[error(transparent)]
    ChatGptError(#[from] chatgpt::err::Error),
}

#[derive(Debug, Config)]
struct Configuration {
    chat_gpt_api_key: String,
    sqlite_database_url: String,
    telegram_bot_api_key: String,
}

async fn request_api_key(
    bot: Bot,
    dialogue: Dialogue<DialogueState, Storage>,
    msg: Message,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    trace!("{:?}", dialogue.get().await?);
    let api_key = msg.text().ok_or("Please provide API key")?.to_owned();

    match dialogue
        .update(DialogueState::Registration { api_key })
        .await
    {
        Ok(()) => {
            info!("{} Success", dialogue.chat_id());
            bot.send_message(msg.chat.id, "Success! You can start conversation!")
                .await?;
        }
        Err(err) => {
            error!("Error while registration: {err:?}");
            bot.send_message(msg.chat.id, "API Key not working, please try again!")
                .await?;
            return Err("Wrong API key".into());
        }
    }

    Ok(())
}

async fn conversation(
    bot: Bot,
    dialogue: Dialogue<DialogueState, Storage>,
    client: ChatGPT,
    msg: Message,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    trace!("{:?}", dialogue.get().await?);
    let Some(DialogueState::Conversation { history }) = dialogue.get().await? else {
        return Err("Internal error".into());
    };
    let msg = msg.text().ok_or("Please provide API key")?.to_owned();
    if msg.starts_with("/reset") {
        dialogue
            .update(DialogueState::Conversation { history: vec![] })
            .await?;
        bot.send_message(dialogue.chat_id(), "Reseted").await?;
        return Ok(());
    }

    trace!(
        "New message from {} in conversation: {}",
        dialogue.chat_id(),
        msg
    );

    let mut conversation = Conversation::new_with_history(client, history);
    bot.send_chat_action(dialogue.chat_id(), teloxide::types::ChatAction::Typing)
        .await?;

    let response = match conversation.send_message(msg).await {
        Ok(response) => response,
        Err(err) => {
            bot.send_message(dialogue.chat_id(), format!("Error while request: {err:?}"))
                .await?;
            return Err(err.into());
        }
    };

    bot.send_message(dialogue.chat_id(), response.message().content.clone())
        .await?;

    dialogue
        .update(DialogueState::Conversation {
            history: conversation.history,
        })
        .await?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt::init();
    let config = Configuration::builder()
        .env()
        .file(env::args().nth(1).ok_or(Error::NoConfigurationFile)?)
        .load()?;

    let bot = teloxide::Bot::new(config.telegram_bot_api_key);

    Dispatcher::builder(
        bot,
        Update::filter_message()
            .enter_dialogue::<Message, Storage, DialogueState>()
            .branch(dptree::case![DialogueState::ApiKeyRequest].endpoint(request_api_key))
            .branch(
                dptree::case![DialogueState::Registration { api_key }].endpoint(request_api_key),
            )
            .branch(dptree::case![DialogueState::Conversation { history }].endpoint(conversation)),
    )
    .dependencies(dptree::deps![
        Arc::new(Storage::try_new(&config.sqlite_database_url).await?),
        ChatGPT::new(config.chat_gpt_api_key)?
    ])
    .enable_ctrlc_handler()
    .build()
    .dispatch()
    .await;

    Ok(())
}
