//! Chats (P4): the `chats` and `chat_messages` tables. A chat is one conversation with a harness; its messages keep their order
//! by `seq` (1, 2, 3, ... per chat). The ledger stores them as plain data; what a role or a card means is the daemon's business.

use serde_json::Value;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use ratchet_core::ledger::LedgerError;

use crate::rowmap::{Result, col, db};
use crate::store::Ledger;

/// A conversation.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatRow {
    pub id: String,
    /// `owner/name` of the repo the chat is about; `None` for a Home chat with no repo.
    pub repo: Option<String>,
    /// The optimization a steering chat is bound to.
    pub optimization_id: Option<String>,
    /// `claude-code`, `codex` or `grok`.
    pub harness: String,
    pub model: Option<String>,
    /// The CLI's own session id, for `--resume` after the live session closed.
    pub cli_session_id: Option<String>,
    /// `open` or `closed`.
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// What `create_chat` needs.
#[derive(Debug, Clone, Default)]
pub struct NewChat {
    pub id: String,
    pub repo: Option<String>,
    pub optimization_id: Option<String>,
    pub harness: String,
    pub model: Option<String>,
}

/// One message of a chat.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessageRow {
    pub chat_id: String,
    pub seq: i64,
    /// `user`, `assistant`, `tool`, `card` or `note`.
    pub role: String,
    pub text: String,
    /// Who wrote an assistant reply.
    pub harness: Option<String>,
    pub model: Option<String>,
    /// The structured payload of a `card` or `note` row.
    pub card: Option<Value>,
    pub usage: Option<Value>,
    pub at: i64,
}

/// What `append_chat_message` needs; the ledger assigns `seq` and `at`.
#[derive(Debug, Clone, Default)]
pub struct NewChatMessage {
    pub role: String,
    pub text: String,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub card: Option<Value>,
    pub usage: Option<Value>,
}

fn chat_row(r: &SqliteRow) -> Result<ChatRow> {
    Ok(ChatRow {
        id: col(r, "id")?,
        repo: col(r, "repo")?,
        optimization_id: col(r, "optimization_id")?,
        harness: col(r, "harness")?,
        model: col(r, "model")?,
        cli_session_id: col(r, "cli_session_id")?,
        status: col(r, "status")?,
        created_at: col(r, "created_at")?,
        updated_at: col(r, "updated_at")?,
    })
}

fn json_col(r: &SqliteRow, name: &str) -> Result<Option<Value>> {
    let text: Option<String> = col(r, name)?;
    text.map(|t| {
        serde_json::from_str(&t)
            .map_err(|e| LedgerError::database(format!("column {name} holds invalid JSON: {e}")))
    })
    .transpose()
}

fn message_row(r: &SqliteRow) -> Result<ChatMessageRow> {
    Ok(ChatMessageRow {
        chat_id: col(r, "chat_id")?,
        seq: col(r, "seq")?,
        role: col(r, "role")?,
        text: col(r, "text")?,
        harness: col(r, "harness")?,
        model: col(r, "model")?,
        card: json_col(r, "card")?,
        usage: json_col(r, "usage")?,
        at: col(r, "at")?,
    })
}

fn to_text(v: &Value) -> Result<String> {
    serde_json::to_string(v)
        .map_err(|e| LedgerError::invalid(format!("cannot store value as JSON: {e}")))
}

impl Ledger {
    pub async fn create_chat(&self, input: NewChat) -> Result<ChatRow> {
        let now = self.now();
        sqlx::query(
            "INSERT INTO chats (id, repo, optimization_id, harness, model, status, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, 'open', ?6, ?6)",
        )
        .bind(&input.id)
        .bind(&input.repo)
        .bind(&input.optimization_id)
        .bind(&input.harness)
        .bind(&input.model)
        .bind(now)
        .execute(&mut *self.conn().await?)
        .await
        .map_err(db)?;
        self.get_chat(&input.id)
            .await?
            .ok_or_else(|| LedgerError::database("the chat vanished after insert"))
    }

    pub async fn get_chat(&self, id: &str) -> Result<Option<ChatRow>> {
        let r = sqlx::query("SELECT * FROM chats WHERE id = ?1")
            .bind(id)
            .fetch_optional(&mut *self.conn().await?)
            .await
            .map_err(db)?;
        r.as_ref().map(chat_row).transpose()
    }

    /// Newest activity first.
    pub async fn list_chats(&self, limit: u32) -> Result<Vec<ChatRow>> {
        let rows = sqlx::query("SELECT * FROM chats ORDER BY updated_at DESC, id LIMIT ?1")
            .bind(i64::from(limit))
            .fetch_all(&mut *self.conn().await?)
            .await
            .map_err(db)?;
        rows.iter().map(chat_row).collect()
    }

    /// The CLI session to resume next time. `None` clears it.
    pub async fn set_chat_cli_session(&self, id: &str, session: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE chats SET cli_session_id = ?2, updated_at = ?3 WHERE id = ?1")
            .bind(id)
            .bind(session)
            .bind(self.now())
            .execute(&mut *self.conn().await?)
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn set_chat_status(&self, id: &str, status: &str) -> Result<()> {
        sqlx::query("UPDATE chats SET status = ?2, updated_at = ?3 WHERE id = ?1")
            .bind(id)
            .bind(status)
            .bind(self.now())
            .execute(&mut *self.conn().await?)
            .await
            .map_err(db)?;
        Ok(())
    }

    /// Remove the chat and, by cascade, its messages. True when it existed.
    pub async fn delete_chat(&self, id: &str) -> Result<bool> {
        let done = sqlx::query("DELETE FROM chats WHERE id = ?1")
            .bind(id)
            .execute(&mut *self.conn().await?)
            .await
            .map_err(db)?;
        Ok(done.rows_affected() > 0)
    }

    /// Append at the end; `seq` is the chat's next number. One statement, so two writers cannot take the same number.
    pub async fn append_chat_message(
        &self,
        chat_id: &str,
        input: NewChatMessage,
    ) -> Result<ChatMessageRow> {
        let now = self.now();
        let card = input.card.as_ref().map(to_text).transpose()?;
        let usage = input.usage.as_ref().map(to_text).transpose()?;
        let mut conn = self.conn().await?;
        let seq: i64 = sqlx::query(
            "INSERT INTO chat_messages (chat_id, seq, role, text, harness, model, card, usage, at) \
             VALUES (?1, COALESCE((SELECT MAX(seq) FROM chat_messages WHERE chat_id = ?1), 0) + 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             RETURNING seq",
        )
        .bind(chat_id)
        .bind(&input.role)
        .bind(&input.text)
        .bind(&input.harness)
        .bind(&input.model)
        .bind(&card)
        .bind(&usage)
        .bind(now)
        .fetch_one(&mut *conn)
        .await
        .map_err(db)?
        .get("seq");
        sqlx::query("UPDATE chats SET updated_at = ?2 WHERE id = ?1")
            .bind(chat_id)
            .bind(now)
            .execute(&mut *conn)
            .await
            .map_err(db)?;
        Ok(ChatMessageRow {
            chat_id: chat_id.to_owned(),
            seq,
            role: input.role,
            text: input.text,
            harness: input.harness,
            model: input.model,
            card: input.card,
            usage: input.usage,
            at: now,
        })
    }

    /// Replace the payload of a `card` or `note` row (a card that changed). False when there is no such row.
    pub async fn set_chat_message_card(
        &self,
        chat_id: &str,
        seq: i64,
        card: &Value,
    ) -> Result<bool> {
        let done = sqlx::query("UPDATE chat_messages SET card = ?3 WHERE chat_id = ?1 AND seq = ?2")
            .bind(chat_id)
            .bind(seq)
            .bind(to_text(card)?)
            .execute(&mut *self.conn().await?)
            .await
            .map_err(db)?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn set_chat_message_usage(
        &self,
        chat_id: &str,
        seq: i64,
        usage: &Value,
    ) -> Result<bool> {
        let done =
            sqlx::query("UPDATE chat_messages SET usage = ?3 WHERE chat_id = ?1 AND seq = ?2")
                .bind(chat_id)
                .bind(seq)
                .bind(to_text(usage)?)
                .execute(&mut *self.conn().await?)
                .await
                .map_err(db)?;
        Ok(done.rows_affected() > 0)
    }

    /// Messages with `seq > after`, oldest first. `limit` caps the page.
    pub async fn chat_messages(
        &self,
        chat_id: &str,
        after: i64,
        limit: u32,
    ) -> Result<Vec<ChatMessageRow>> {
        let rows = sqlx::query(
            "SELECT * FROM chat_messages WHERE chat_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
        )
        .bind(chat_id)
        .bind(after)
        .bind(i64::from(limit))
        .fetch_all(&mut *self.conn().await?)
        .await
        .map_err(db)?;
        rows.iter().map(message_row).collect()
    }

    /// The newest `limit` messages, oldest first (a chat's tail).
    pub async fn chat_messages_tail(
        &self,
        chat_id: &str,
        limit: u32,
    ) -> Result<Vec<ChatMessageRow>> {
        let rows = sqlx::query(
            "SELECT * FROM (SELECT * FROM chat_messages WHERE chat_id = ?1 ORDER BY seq DESC LIMIT ?2) ORDER BY seq",
        )
        .bind(chat_id)
        .bind(i64::from(limit))
        .fetch_all(&mut *self.conn().await?)
        .await
        .map_err(db)?;
        rows.iter().map(message_row).collect()
    }

    /// The `seq` of the newest message (0 for none).
    pub async fn last_chat_seq(&self, chat_id: &str) -> Result<i64> {
        let r = sqlx::query("SELECT COALESCE(MAX(seq), 0) AS n FROM chat_messages WHERE chat_id = ?1")
            .bind(chat_id)
            .fetch_one(&mut *self.conn().await?)
            .await
            .map_err(db)?;
        col(&r, "n")
    }

    /// The row of the card or note whose payload has `"id": card_id`.
    pub async fn find_chat_card(
        &self,
        chat_id: &str,
        card_id: &str,
    ) -> Result<Option<ChatMessageRow>> {
        let r = sqlx::query(
            "SELECT * FROM chat_messages WHERE chat_id = ?1 AND card IS NOT NULL \
             AND json_extract(card, '$.id') = ?2 ORDER BY seq DESC LIMIT 1",
        )
        .bind(chat_id)
        .bind(card_id)
        .fetch_optional(&mut *self.conn().await?)
        .await
        .map_err(db)?;
        r.as_ref().map(message_row).transpose()
    }
}
