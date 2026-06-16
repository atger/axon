pub mod message;
pub use message::{Message, Role};

pub struct ConversationHistory {
    messages: Vec<Message>,
    context_window: usize,
}

impl ConversationHistory {
    pub fn new(context_window: usize) -> Self {
        Self {
            messages: Vec::new(),
            context_window,
        }
    }

    pub fn push(&mut self, msg: Message) {
        self.messages.push(msg);
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn set_context_window(&mut self, cw: usize) {
        self.context_window = cw;
    }

    pub fn clear(&mut self) {
        self.messages.retain(|m| m.role == Role::System);
    }

    /// Assembles messages within the token budget, keeping the system prompt
    /// and dropping the oldest user/assistant pairs when over budget.
    pub fn assemble(&self, system_prompt: &str) -> Vec<Message> {
        let mut result = vec![Message::system(system_prompt)];
        let budget = self
            .context_window
            .saturating_sub(system_prompt.len() / 4 + 1);
        let mut used = 0usize;

        let mut kept: Vec<Message> = Vec::new();
        for msg in self
            .messages
            .iter()
            .filter(|m| m.role != Role::System)
            .rev()
        {
            let cost = msg.content.len() / 4 + 1;
            if used + cost > budget {
                break;
            }
            used += cost;
            kept.push(msg.clone());
        }
        kept.reverse();
        result.append(&mut kept);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assemble_respects_budget() {
        let mut h = ConversationHistory::new(10);
        // Each message costs ~1 token (4 chars / 4 + 1)
        for i in 0..20u32 {
            h.push(Message::user(format!("{i}")));
        }
        let assembled = h.assemble("sys");
        // Should have system prompt + some messages, not all 20
        assert!(assembled.len() < 22);
        assert_eq!(assembled[0].role, Role::System);
    }

    #[test]
    fn clear_keeps_system_messages() {
        let mut h = ConversationHistory::new(2048);
        h.push(Message::system("sys"));
        h.push(Message::user("hi"));
        h.push(Message::assistant("hello"));
        h.clear();
        assert_eq!(h.messages.len(), 1);
        assert_eq!(h.messages[0].role, Role::System);
    }
}
