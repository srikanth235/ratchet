//! Chat (P4): `ChatService` and what it stands on.
//!
//! * [`service`]: chats, the live-session pool, turns, streams, cards (the heart);
//! * [`engine`]: what the service needs from the machine (harnesses, checkouts, sessions) as a trait;
//! * [`scripted`]: the engine with no model, for simulation, snapshots and tests;
//! * [`cards`]: draft cards, steering notes and the words of errors;
//! * [`prompt`]: the role prompt; [`queue`]: the sink-to-pump hand-off.
//!
//! The HTTP side is `crate::routes::chats`; the wire types are `crate::api::chat`. `docs/ratchet-chat.md` has the picture.

pub mod cards;
pub mod engine;
pub mod prompt;
pub mod queue;
pub mod scripted;
pub mod service;

pub use engine::{ChatEngine, ChatHarness, ChatToolCaller, ToolScope};
pub use scripted::{ScriptedChatEngine, ScriptedOptions};
pub use service::{ChatOptions, ChatService, ChatStream, DEFAULT_HARNESS_KEY};
