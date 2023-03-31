use chatgpt::prelude::ChatMessage;
use futures::future::BoxFuture;
use sha3::{Digest, Keccak256};
use std::sync::Arc;
use teloxide::{dispatching::dialogue::Storage, prelude::*};
use tracing::*;

pub use chatgpt::config::ChatGPTEngine;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    HistoryCorrupted(#[from] serde_json::Error),
    #[error(transparent)]
    DB(#[from] sqlx::Error),
    #[error(transparent)]
    Migration(#[from] sqlx::migrate::MigrateError),
}

#[derive(Debug, Default, Clone)]
pub enum DialogueState {
    #[default]
    ApiKeyRequest,
    Registration {
        api_key: String,
    },
    Conversation {
        history: Vec<ChatMessage>,
        version: ChatGPTEngine,
    },
}

pub struct BotState {
    pub pool: sqlx::SqlitePool,
}
impl BotState {
    pub async fn try_new(sqlite_database_url: &str) -> Result<Self, Error> {
        let self_ = Self {
            pool: sqlx::SqlitePool::connect(sqlite_database_url).await?,
        };

        sqlx::migrate!().run(&self_.pool).await?;

        Ok(self_)
    }
}

impl Storage<DialogueState> for BotState {
    type Error = Error;

    fn remove_dialogue(
        self: Arc<Self>,
        chat_id: ChatId,
    ) -> BoxFuture<'static, Result<(), Self::Error>> {
        Box::pin(async move {
            sqlx::query!(r#"DELETE FROM "users" WHERE "chat_id" = ?"#, chat_id.0)
                .execute(&self.pool)
                .await
                .map_err(Error::DB)?;
            Ok(())
        })
    }

    fn update_dialogue(
        self: Arc<Self>,
        chat_id: ChatId,
        dialogue: DialogueState,
    ) -> BoxFuture<'static, Result<(), Self::Error>> {
        trace!("{dialogue:?}");
        match dialogue {
            DialogueState::ApiKeyRequest => Box::pin(async { Ok(()) }),
            DialogueState::Registration { api_key } => {
                let api_hash: Vec<u8> = Keccak256::digest(&api_key).into_iter().collect();
                let api_prefix: Vec<u8> = api_key.as_bytes().iter().take(10).copied().collect();

                Box::pin(async move {
                    sqlx::query!(r#"
                        INSERT INTO "users" ("chat_id", "current_prompt", "history", "api_key_prefix")
                        SELECT ?, ?, ?, ?
                        WHERE EXISTS(SELECT * FROM "api_keys" WHERE "key_hash" = ? AND "key_prefix" = ?)"#,
                        chat_id.0,
                        "",
                        "[]",
                        api_prefix,
                        api_hash,
                        api_prefix,
                    )
                    .execute(&self.pool)
                    .await
                    .map_err(Error::DB)?;
                    Ok(())
                })
            }
            DialogueState::Conversation { history, version } => Box::pin(async move {
                let history_json = serde_json::to_string(&history)?;

                let version_ref = version.as_ref();
                sqlx::query!(
                    r#"UPDATE "users" SET "history" = ?, "version" = ? WHERE "chat_id" = ?"#,
                    history_json,
                    version_ref,
                    chat_id.0,
                )
                .execute(&self.pool)
                .await
                .map_err(Error::DB)?;

                Ok(())
            }),
        }
    }

    fn get_dialogue(
        self: Arc<Self>,
        chat_id: ChatId,
    ) -> BoxFuture<'static, Result<Option<DialogueState>, Self::Error>> {
        Box::pin(async move {
            sqlx::query!(
                r#"SELECT "history", "version" FROM "users" WHERE "chat_id" = ?"#,
                chat_id.0,
            )
            .fetch_optional(&self.pool)
            .await?
            .map(|row| {
                trace!("{row:?}");

                Ok(DialogueState::Conversation {
                    history: serde_json::from_str(&row.history)?,
                    version: match row.version.as_str() {
                        "gpt-3.5-turbo" => ChatGPTEngine::Gpt35Turbo,
                        "gpt-3.5-turbo-0301" => ChatGPTEngine::Gpt35Turbo_0301,
                        "gpt-4" => ChatGPTEngine::Gpt4,
                        "gpt-4-32k" => ChatGPTEngine::Gpt4_32k,
                        "gpt-4-0314" => ChatGPTEngine::Gpt4_0314,
                        "gpt-4-32k-0314" => ChatGPTEngine::Gpt4_32k_0314,
                        _custom => ChatGPTEngine::Gpt35Turbo,
                    },
                })
            })
            .transpose()
        })
    }
}
