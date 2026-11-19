mod cloud;
mod credentials;
mod local;
pub(crate) mod server;

pub use cloud::process_cloud;
pub use credentials::{has_api_key, set_api_key};
pub use local::process_local;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

pub const DEFAULT_PROCESSING_PROMPT: &str = "Отредактируй фразу как корректор. Убери слова-паразиты и случайные повторы. Начни предложение с заглавной буквы, заверши подходящим знаком. Сохрани исходный язык, слова, обращение на ты или вы и смысл. Верни только исправленную фразу.";
pub const MAX_PROMPT_BYTES: usize = 8192;
const MAX_INPUT: usize = 120_000;
const MAX_OUTPUT: usize = 128 * 1024;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum Provider {
    #[default]
    OpenAI,
    Anthropic,
}

impl Provider {
    pub fn models(self) -> &'static [&'static str] {
        match self {
            Self::OpenAI => &["gpt-6-luna", "gpt-4o-mini", "gpt-4o"],
            Self::Anthropic => &["claude-haiku-5-5", "claude-sonnet-5-5"],
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::OpenAI => "OpenAI",
            Self::Anthropic => "Anthropic",
        }
    }
}

pub fn validate_processing_prompt(prompt: &str) -> Result<()> {
    ensure!(!prompt.trim().is_empty(), "Введите инструкцию обработки");
    ensure!(
        prompt.len() <= MAX_PROMPT_BYTES,
        "Инструкция превышает 8192 байта"
    );
    ensure!(
        !prompt.contains('\0'),
        "Инструкция содержит недопустимый нулевой символ"
    );
    Ok(())
}

fn validate_input(text: &str) -> Result<()> {
    ensure!(
        !text.trim().is_empty() && text.len() <= MAX_INPUT,
        "Транскрипт пуст или превышает допустимую длину"
    );
    Ok(())
}

fn edited(output: &str) -> Result<String> {
    let text = output.trim();
    ensure!(
        !text.is_empty() && text.len() <= MAX_OUTPUT,
        "Провайдер вернул пустой или слишком большой результат"
    );
    Ok(text.into())
}
