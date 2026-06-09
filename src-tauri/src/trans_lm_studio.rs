use crate::config::Config;
use crate::translation::{
    TranslationProvider, TranslationResult, clean_text_for_translation, create_smart_prompt,
};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};

/// LM Studio exposes an OpenAI-compatible API on a local server (default
/// http://127.0.0.1:1234). This provider mirrors the OpenAI chat flow but with a
/// configurable base URL and no auth header. LM Studio runs local models (not OpenAI
/// reasoning models), so the reasoning-model branches from the OpenAI provider are omitted
/// and the standard chat path is always used.
pub struct LmStudioTranslationService {
    client: reqwest::Client,
    config: Config,
}

impl LmStudioTranslationService {
    pub fn new(config: Config) -> Self {
        log::info!(
            "Creating LmStudioTranslationService with URL: {}, model: {}",
            config
                .lm_studio_url
                .as_deref()
                .unwrap_or("http://127.0.0.1:1234"),
            config.model
        );
        Self {
            client: reqwest::Client::new(),
            config,
        }
    }

    fn chat_completions_url(&self) -> String {
        let base = self
            .config
            .lm_studio_url
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("http://127.0.0.1:1234");
        format!("{}/v1/chat/completions", base.trim_end_matches('/'))
    }

    async fn call_lm_studio(&self, request_body: Value) -> Result<Value> {
        let url = self.chat_completions_url();

        log::info!("Making LM Studio request to: {}", url);
        log::info!(
            "Request body: {}",
            serde_json::to_string_pretty(&request_body).unwrap_or_default()
        );

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await?;

        if response.status().is_success() {
            return Ok(response.json().await?);
        }

        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        log::error!("LM Studio API request failed ({}): {}", status, error_text);
        Err(anyhow::anyhow!(
            "LM Studio API request failed ({}): {}",
            status,
            error_text
        ))
    }

    fn parse_response_content(&self, content: &str) -> Result<TranslationResult> {
        log::info!("LM Studio Response content: {}", content);

        // Clean the content by removing control characters that can break JSON parsing
        let cleaned_content = content
            .chars()
            .filter(|c| !c.is_control() || matches!(*c, '\n' | '\r' | '\t'))
            .collect::<String>();

        if cleaned_content != content {
            log::warn!("Removed control characters from LM Studio response");
        }

        // Try to parse as JSON, but handle cases where the model returned plain text
        let parsed: Value = match serde_json::from_str(&cleaned_content) {
            Ok(json) => json,
            Err(parse_error) => {
                log::warn!("Failed to parse LM Studio response as JSON: {}", parse_error);

                // Try to find and extract valid JSON from the response
                if let Some(start_idx) = cleaned_content.find('{') {
                    let mut brace_count = 0;
                    let mut end_idx = None;

                    for (i, c) in cleaned_content[start_idx..].char_indices() {
                        if c == '{' {
                            brace_count += 1;
                        } else if c == '}' {
                            brace_count -= 1;
                            if brace_count == 0 {
                                end_idx = Some(start_idx + i + 1);
                                break;
                            }
                        }
                    }

                    if let Some(end_idx) = end_idx {
                        let json_str = &cleaned_content[start_idx..end_idx];
                        match serde_json::from_str::<Value>(json_str) {
                            Ok(json) => json,
                            Err(e) => {
                                log::warn!("Failed to parse extracted JSON from LM Studio: {}", e);
                                json!({
                                    "detected_language": "unknown",
                                    "translated_text": cleaned_content
                                })
                            }
                        }
                    } else {
                        json!({
                            "detected_language": "unknown",
                            "translated_text": cleaned_content
                        })
                    }
                } else {
                    json!({
                        "detected_language": "unknown",
                        "translated_text": cleaned_content
                    })
                }
            }
        };

        let detected_language = match parsed["detected_language"].as_str() {
            Some(lang) if !lang.is_empty() => lang.to_string(),
            _ => "unknown".to_string(),
        };
        let translated_text = match parsed["translated_text"].as_str() {
            Some(text) => text
                .replace("\\n", "\n")
                .replace("\\r\\n", "\n")
                .replace("\\r", "\n")
                .replace("\\t", "\t"),
            None => {
                if parsed.is_string() {
                    parsed
                        .as_str()
                        .unwrap_or("translation failed")
                        .replace("\\n", "\n")
                        .replace("\\r\\n", "\n")
                        .replace("\\r", "\n")
                        .replace("\\t", "\t")
                } else {
                    "translation failed".to_string()
                }
            }
        };

        log::info!(
            "Detected language (provider reported): {}",
            detected_language
        );

        Ok(TranslationResult {
            detected_language,
            translated_text,
            target_language: self.config.target_language.clone(),
        })
    }
}

#[async_trait]
impl TranslationProvider for LmStudioTranslationService {
    async fn translate(&self, text: &str) -> Result<TranslationResult> {
        let cleaned_text = clean_text_for_translation(text);
        log::info!("Cleaned text for LM Studio translation: {}", cleaned_text);

        if self.config.model.trim().is_empty() {
            log::error!("LmStudioTranslationService: model is empty; cannot proceed");
            return Err(anyhow::anyhow!(
                "Model not configured for LM Studio provider"
            ));
        }

        // Check if this is an alternatives request (custom_prompt contains "alternatives")
        let is_alternatives_request = self.config.custom_prompt.contains("alternatives");

        let (user_prompt, system_content) = if is_alternatives_request {
            log::info!("Using alternatives prompt for LM Studio");
            (
                format!("\"{}\"", cleaned_text),
                self.config.custom_prompt.clone(),
            )
        } else {
            let user_prompt = format!(
                "Text to translate into {}: \"{}\"",
                self.config.target_language, cleaned_text
            );
            let smart_prompt = create_smart_prompt(&self.config, None);
            let system_content = format!(
                "{}\n\nIMPORTANT FORMATTING RULES:\n- Always respond with valid JSON containing 'detected_language' and 'translated_text' fields\n- Preserve line breaks and paragraph structure in the translation\n- Use actual newline characters in the JSON string value\n\nExample response format:\n{{\n  \"detected_language\": \"English\",\n  \"translated_text\": \"Line 1\\nLine 2\"\n}}",
                smart_prompt
            );
            (user_prompt, system_content)
        };

        let request_body = json!({
            "model": self.config.model,
            "messages": [
                { "role": "system", "content": system_content },
                { "role": "user", "content": user_prompt }
            ],
            "max_tokens": 800,
            "temperature": 0.3,
        });

        log::info!("Using LM Studio model: {}", self.config.model);

        let response = self.call_lm_studio(request_body).await?;

        let choices = response["choices"].as_array().ok_or_else(|| {
            anyhow::anyhow!(
                "No 'choices' array in LM Studio response. Full response: {}",
                serde_json::to_string_pretty(&response).unwrap_or_default()
            )
        })?;

        if choices.is_empty() {
            return Err(anyhow::anyhow!("Empty 'choices' array in LM Studio response"));
        }

        let content = choices[0]["message"]["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No 'content' field in LM Studio message"))?;

        if is_alternatives_request {
            // For alternatives requests, return the raw content; the caller parses it.
            log::info!("Returning raw alternatives response from LM Studio");
            Ok(TranslationResult {
                detected_language: "unknown".to_string(),
                translated_text: content.to_string(),
                target_language: "alternatives".to_string(),
            })
        } else {
            self.parse_response_content(content)
        }
    }
}
