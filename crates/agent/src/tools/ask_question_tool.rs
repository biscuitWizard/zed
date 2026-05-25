use crate::{AgentTool, ToolCallEventStream, ToolInput};
use agent_client_protocol::schema as acp;
use gpui::{App, SharedString, Task};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

fn default_true() -> bool {
    true
}

/// Ask the user one or more structured clarifying questions.
///
/// Use this tool when you need information from the user before proceeding,
/// and the answer can be expressed as one of a small set of choices. The user
/// sees a form with buttons/checkboxes for each option, plus an optional
/// free-text "More details" area.
///
/// When to use:
/// - You need the user to choose between distinct alternatives.
/// - You have 1-5 questions that can each be answered with a short list of options.
///
/// When NOT to use:
/// - The answer is open-ended (use a plain-text question in your message instead).
/// - You can determine the answer yourself from context, code, or prior messages.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AskQuestionToolInput {
    /// Optional title displayed at the top of the question form.
    #[serde(default)]
    pub title: Option<String>,

    /// The questions to ask. At least one is required.
    pub questions: Vec<Question>,

    /// When true, the form includes a free-text "More details" textarea
    /// so the user can add context beyond the structured choices.
    /// Defaults to true.
    #[serde(default = "default_true")]
    pub allow_free_text_details: bool,

    /// Optional placeholder text shown in the "More details" textarea.
    #[serde(default)]
    pub free_text_details_placeholder: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Question {
    /// Unique identifier for this question, used as the key in the response.
    pub id: String,

    /// The question text displayed to the user.
    #[serde(alias = "prompt")]
    pub question: String,

    /// The answer options presented to the user. At least 2 required.
    pub options: Vec<QuestionOption>,

    /// If true, the user can select multiple options. Defaults to false (single-select).
    #[serde(default)]
    pub allow_multiple: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QuestionOption {
    /// Unique identifier for this option within its question.
    pub id: String,

    /// Display label shown to the user.
    pub label: String,

    /// Optional description rendered as subtext under the label.
    /// Use this to explain what choosing this option implies so the user
    /// can make an informed choice.
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskQuestionToolOutput {
    pub answers: HashMap<String, Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl From<AskQuestionToolOutput> for language_model::LanguageModelToolResultContent {
    fn from(output: AskQuestionToolOutput) -> Self {
        let json = serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string());
        language_model::LanguageModelToolResultContent::Text(json.into())
    }
}

pub struct AskQuestionTool;

impl AgentTool for AskQuestionTool {
    type Input = AskQuestionToolInput;
    type Output = AskQuestionToolOutput;

    const NAME: &'static str = "ask_question";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Think
    }

    fn initial_title(
        &self,
        input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        match input {
            Ok(input) => {
                if let Some(title) = &input.title {
                    SharedString::from(format!("Question: {title}"))
                } else if let Some(first) = input.questions.first() {
                    let q = &first.question;
                    if q.len() > 50 {
                        SharedString::from(format!("Question: {}…", &q[..47]))
                    } else {
                        SharedString::from(format!("Question: {q}"))
                    }
                } else {
                    "Ask question".into()
                }
            }
            Err(_) => "Ask question".into(),
        }
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<Self::Output, Self::Output>> {
        cx.spawn(async move |cx| {
            let input = input.recv().await.map_err(|e| AskQuestionToolOutput {
                answers: HashMap::new(),
                details: Some(e.to_string()),
            })?;

            if input.questions.is_empty() {
                return Err(AskQuestionToolOutput {
                    answers: HashMap::new(),
                    details: Some("At least one question is required.".to_string()),
                });
            }

            for question in &input.questions {
                if question.options.len() < 2 {
                    return Err(AskQuestionToolOutput {
                        answers: HashMap::new(),
                        details: Some(format!(
                            "Question '{}' must have at least 2 options.",
                            question.id
                        )),
                    });
                }
            }

            let questions: Vec<acp_thread::MultiChoiceQuestion> = input
                .questions
                .iter()
                .map(|q| acp_thread::MultiChoiceQuestion {
                    id: q.id.clone(),
                    question: q.question.clone(),
                    options: q
                        .options
                        .iter()
                        .map(|o| acp_thread::MultiChoiceOption {
                            id: o.id.clone(),
                            label: o.label.clone(),
                            description: o.description.clone(),
                        })
                        .collect(),
                    allow_multiple: q.allow_multiple,
                })
                .collect();

            let task = cx.update(|cx| {
                event_stream.prompt_for_questions(
                    questions,
                    input.title.clone(),
                    input.allow_free_text_details,
                    input.free_text_details_placeholder.clone(),
                    cx,
                )
            });

            match task.await {
                Ok(outcome) => Ok(AskQuestionToolOutput {
                    answers: outcome.answers,
                    details: outcome.details,
                }),
                Err(e) => Err(AskQuestionToolOutput {
                    answers: HashMap::new(),
                    details: Some(e.to_string()),
                }),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ToolCallEventStream, ToolInput};
    use acp_thread::MultiChoiceOutcome;
    use gpui::TestAppContext;

    fn single_question_input() -> AskQuestionToolInput {
        AskQuestionToolInput {
            title: Some("Pick a framework".to_string()),
            questions: vec![Question {
                id: "framework".to_string(),
                question: "Which framework should we use?".to_string(),
                options: vec![
                    QuestionOption {
                        id: "react".to_string(),
                        label: "React".to_string(),
                        description: Some("Component-based UI library".to_string()),
                    },
                    QuestionOption {
                        id: "vue".to_string(),
                        label: "Vue".to_string(),
                        description: Some("Progressive framework".to_string()),
                    },
                ],
                allow_multiple: false,
            }],
            allow_free_text_details: true,
            free_text_details_placeholder: Some("Any additional context?".to_string()),
        }
    }

    fn multi_question_input() -> AskQuestionToolInput {
        AskQuestionToolInput {
            title: None,
            questions: vec![
                Question {
                    id: "language".to_string(),
                    question: "Preferred language?".to_string(),
                    options: vec![
                        QuestionOption {
                            id: "rust".to_string(),
                            label: "Rust".to_string(),
                            description: None,
                        },
                        QuestionOption {
                            id: "typescript".to_string(),
                            label: "TypeScript".to_string(),
                            description: None,
                        },
                    ],
                    allow_multiple: false,
                },
                Question {
                    id: "features".to_string(),
                    question: "Select features".to_string(),
                    options: vec![
                        QuestionOption {
                            id: "auth".to_string(),
                            label: "Authentication".to_string(),
                            description: Some("OAuth2 + session management".to_string()),
                        },
                        QuestionOption {
                            id: "db".to_string(),
                            label: "Database".to_string(),
                            description: Some("PostgreSQL with migrations".to_string()),
                        },
                        QuestionOption {
                            id: "cache".to_string(),
                            label: "Caching".to_string(),
                            description: None,
                        },
                    ],
                    allow_multiple: true,
                },
            ],
            allow_free_text_details: true,
            free_text_details_placeholder: None,
        }
    }

    #[gpui::test]
    async fn test_single_question(cx: &mut TestAppContext) {
        let tool = Arc::new(AskQuestionTool);
        let (event_stream, mut event_rx) = ToolCallEventStream::test();

        let input = single_question_input();
        let task = cx.update(|cx| tool.run(ToolInput::resolved(input), event_stream, cx));

        let auth = event_rx.expect_multi_choice_authorization().await;

        // Verify the emitted options structure
        if let acp_thread::PermissionOptions::MultiChoice {
            questions,
            title,
            allow_free_text_details,
            ..
        } = &auth.options
        {
            assert_eq!(title.as_deref(), Some("Pick a framework"));
            assert!(allow_free_text_details);
            assert_eq!(questions.len(), 1);
            assert_eq!(questions[0].id, "framework");
            assert_eq!(questions[0].options.len(), 2);
            assert_eq!(
                questions[0].options[0].description.as_deref(),
                Some("Component-based UI library")
            );
        } else {
            panic!("Expected MultiChoice options");
        }

        // Simulate user response
        let mut answers = HashMap::new();
        answers.insert("framework".to_string(), vec!["react".to_string()]);
        auth.response
            .send(MultiChoiceOutcome {
                answers: answers.clone(),
                details: Some("I prefer hooks".to_string()),
            })
            .unwrap();

        let result = task.await.expect("tool should succeed");
        assert_eq!(result.answers, answers);
        assert_eq!(result.details.as_deref(), Some("I prefer hooks"));
    }

    #[gpui::test]
    async fn test_multi_question_allow_multiple(cx: &mut TestAppContext) {
        let tool = Arc::new(AskQuestionTool);
        let (event_stream, mut event_rx) = ToolCallEventStream::test();

        let input = multi_question_input();
        let task = cx.update(|cx| tool.run(ToolInput::resolved(input), event_stream, cx));

        let auth = event_rx.expect_multi_choice_authorization().await;

        if let acp_thread::PermissionOptions::MultiChoice { questions, .. } = &auth.options {
            assert_eq!(questions.len(), 2);
            assert!(!questions[0].allow_multiple);
            assert!(questions[1].allow_multiple);
        } else {
            panic!("Expected MultiChoice options");
        }

        let mut answers = HashMap::new();
        answers.insert("language".to_string(), vec!["rust".to_string()]);
        answers.insert(
            "features".to_string(),
            vec!["auth".to_string(), "db".to_string()],
        );
        auth.response
            .send(MultiChoiceOutcome {
                answers: answers.clone(),
                details: None,
            })
            .unwrap();

        let result = task.await.expect("tool should succeed");
        assert_eq!(result.answers, answers);
        assert_eq!(result.details, None);
    }

    #[gpui::test]
    async fn test_empty_questions_returns_error(cx: &mut TestAppContext) {
        let tool = Arc::new(AskQuestionTool);
        let (event_stream, _event_rx) = ToolCallEventStream::test();

        let input = AskQuestionToolInput {
            title: None,
            questions: vec![],
            allow_free_text_details: false,
            free_text_details_placeholder: None,
        };
        let result = cx
            .update(|cx| tool.run(ToolInput::resolved(input), event_stream, cx))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.details.as_ref().unwrap().contains("At least one question"));
    }

    #[gpui::test]
    async fn test_too_few_options_returns_error(cx: &mut TestAppContext) {
        let tool = Arc::new(AskQuestionTool);
        let (event_stream, _event_rx) = ToolCallEventStream::test();

        let input = AskQuestionToolInput {
            title: None,
            questions: vec![Question {
                id: "q1".to_string(),
                question: "Choose".to_string(),
                options: vec![QuestionOption {
                    id: "only".to_string(),
                    label: "Only option".to_string(),
                    description: None,
                }],
                allow_multiple: false,
            }],
            allow_free_text_details: false,
            free_text_details_placeholder: None,
        };
        let result = cx
            .update(|cx| tool.run(ToolInput::resolved(input), event_stream, cx))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.details.as_ref().unwrap().contains("at least 2 options"));
    }

    #[gpui::test]
    async fn test_details_roundtrip_when_disabled(cx: &mut TestAppContext) {
        let tool = Arc::new(AskQuestionTool);
        let (event_stream, mut event_rx) = ToolCallEventStream::test();

        let input = AskQuestionToolInput {
            title: None,
            questions: vec![Question {
                id: "q".to_string(),
                question: "Pick one".to_string(),
                options: vec![
                    QuestionOption {
                        id: "a".to_string(),
                        label: "A".to_string(),
                        description: None,
                    },
                    QuestionOption {
                        id: "b".to_string(),
                        label: "B".to_string(),
                        description: None,
                    },
                ],
                allow_multiple: false,
            }],
            allow_free_text_details: false,
            free_text_details_placeholder: None,
        };
        let task = cx.update(|cx| tool.run(ToolInput::resolved(input), event_stream, cx));

        let auth = event_rx.expect_multi_choice_authorization().await;

        if let acp_thread::PermissionOptions::MultiChoice {
            allow_free_text_details,
            ..
        } = &auth.options
        {
            assert!(!allow_free_text_details);
        } else {
            panic!("Expected MultiChoice options");
        }

        let mut answers = HashMap::new();
        answers.insert("q".to_string(), vec!["a".to_string()]);
        auth.response
            .send(MultiChoiceOutcome {
                answers: answers.clone(),
                details: None,
            })
            .unwrap();

        let result = task.await.expect("tool should succeed");
        assert_eq!(result.answers, answers);
        assert_eq!(result.details, None);
    }
}
