use super::{ChatMessage, LLMContext, ToolDefinition};

impl LLMContext {
    pub fn new(id: impl Into<String>, system_prompt: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            version: 0,
            system_prompt: system_prompt.into(),
            global_prompt: Vec::new(),
            messages: Vec::new(),
            tools: Vec::new(),
        }
    }

    pub fn append(&mut self, message: ChatMessage) {
        self.messages.push(message);
        self.version += 1;
    }

    pub fn set_tools(&mut self, tools: Vec<ToolDefinition>) {
        self.tools = tools;
        self.version += 1;
    }
}
