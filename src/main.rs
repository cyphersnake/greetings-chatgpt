use std::{env, sync::Arc};

use chatgpt::prelude::{ChatGPT, Conversation};
use confique::Config;
use teloxide::prelude::*;
use tracing::*;

mod bot_state;
use bot_state::{BotState as Storage, ChatGPTEngine, DialogueState};

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
    mut client: ChatGPT,
    msg: Message,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    trace!("{:?}", dialogue.get().await?);

    let Some(DialogueState::Conversation { history, version }) = dialogue.get().await? else {
        return Err("Internal error".into());
    };
    let msg = msg.text().ok_or("Please provide API key")?.to_owned();
    if msg.starts_with("/reset") {
        dialogue
            .update(DialogueState::Conversation {
                history: vec![],
                version,
            })
            .await?;
        bot.send_message(dialogue.chat_id(), "✖️ History Reseted")
            .await?;
        return Ok(());
    }
    if msg.starts_with("/tail") {
        dialogue
            .update(DialogueState::Conversation {
                history: history.into_iter().skip(1).collect(),
                version,
            })
            .await?;
        bot.send_message(dialogue.chat_id(), "✖️ Take Tail").await?;
        return Ok(());
    }
    if msg.starts_with("/gpt3") {
        dialogue
            .update(DialogueState::Conversation {
                history,
                version: ChatGPTEngine::Gpt35Turbo,
            })
            .await?;
        bot.send_message(dialogue.chat_id(), "🕹GPT-3.5").await?;
        return Ok(());
    }
    if msg.starts_with("/gpt4") {
        dialogue
            .update(DialogueState::Conversation {
                history,
                version: ChatGPTEngine::Gpt4,
            })
            .await?;
        bot.send_message(dialogue.chat_id(), "🕹GPT-4").await?;
        return Ok(());
    }

    trace!(
        "New message from {} in conversation: {}",
        dialogue.chat_id(),
        msg
    );

    client.config.engine = version;
    let mut conversation = Conversation::new_with_history(client, history);

    let task = {
        let bot = bot.clone();
        let chat_id = dialogue.chat_id();
        tokio::task::spawn(async move {
            loop {
                _ = bot
                    .send_chat_action(chat_id, teloxide::types::ChatAction::Typing)
                    .await;
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            }
        })
    };

    let res = conversation.send_message(msg).await;
    task.abort();

    let response = match res {
        Ok(response) => response,
        Err(err) => {
            bot.send_message(
                dialogue.chat_id(),
                format!("Error while request: {err:?}, You can try call /reset or /tail"),
            )
            .await?;
            return Err(err.into());
        }
    };

    bot.send_message(dialogue.chat_id(), response.message().content.clone())
        .await?;

    dialogue
        .update(DialogueState::Conversation {
            history: conversation.history,
            version,
        })
        .await?;

    Ok(())
}

pub async fn insert_api_key(api_key: &str, storage: &Storage) -> Result<(), sqlx::Error> {
    use sha3::Digest;
    let api_hash: Vec<u8> = sha3::Keccak256::digest(api_key).into_iter().collect();
    let api_prefix: Vec<u8> = api_key.as_bytes().iter().take(10).copied().collect();

    sqlx::query!(
        r#" INSERT INTO "api_keys" ("key_hash", "key_prefix") VALUES (?, ?)"#,
        api_hash,
        api_prefix
    )
    .execute(&storage.pool)
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

    let storage = Storage::try_new(&config.sqlite_database_url).await?;
    Dispatcher::builder(
        bot,
        Update::filter_message()
            .enter_dialogue::<Message, Storage, DialogueState>()
            .branch(dptree::case![DialogueState::ApiKeyRequest].endpoint(request_api_key))
            .branch(
                dptree::case![DialogueState::Registration { api_key }].endpoint(request_api_key),
            )
            .branch(
                dptree::case![DialogueState::Conversation { history, version }]
                    .endpoint(conversation),
            ),
    )
    .dependencies(dptree::deps![
        Arc::new(storage),
        ChatGPT::new(config.chat_gpt_api_key)?
    ])
    .enable_ctrlc_handler()
    .build()
    .dispatch()
    .await;

    Ok(())
}
