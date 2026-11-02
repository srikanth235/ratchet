//! A session's tools: one [`ToolSet`], enforced here, run by a [`ToolBackend`].

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::sets::ToolSet;
use crate::tool::{Refusal, Tool, ToolBackend, ToolOutput, ToolProvider};

pub struct SessionTools {
    set: ToolSet,
    backend: Arc<dyn ToolBackend>,
}

impl SessionTools {
    pub fn new(set: ToolSet, backend: Arc<dyn ToolBackend>) -> Self {
        Self { set, backend }
    }

    pub fn set(&self) -> ToolSet {
        self.set
    }
}

#[async_trait]
impl ToolProvider for SessionTools {
    fn tools(&self) -> Vec<Tool> {
        self.set.tools()
    }

    async fn call(&self, name: &str, arguments: Value) -> Result<ToolOutput, Refusal> {
        let Some(tool) = self.set.find(name) else {
            return Err(Refusal(format!(
                "tool \"{name}\" is not available in this session"
            )));
        };
        if let Some(validate) = tool.validate
            && let Err(message) = validate(&arguments)
        {
            return Ok(ToolOutput::error(message));
        }
        Ok(match self.backend.call(name, arguments).await {
            Ok(v) => ToolOutput::json(v),
            Err(e) => ToolOutput::error(e.0),
        })
    }
}
