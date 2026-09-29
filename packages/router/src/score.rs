//! The router: ranks every registered model for a task by rules, without
//! spending tokens (ADR-0011).

use crate::activity::{detect, normalize, words, Activity};
use crate::catalog::CatalogModel;
use orchestrator_core::ProviderId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// What matters most when picking a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Preference {
    Quality,
    Balanced,
    Cost,
    Speed,
}

impl Preference {
    pub fn label(self) -> &'static str {
        match self {
            Preference::Quality => "qualidade",
            Preference::Balanced => "equilíbrio",
            Preference::Cost => "custo",
            Preference::Speed => "velocidade",
        }
    }

    /// Weights of (tags, quality, cost, speed, context, tools).
    fn weights(self) -> [f64; 6] {
        match self {
            Preference::Quality => [0.30, 0.35, 0.05, 0.00, 0.15, 0.15],
            Preference::Balanced => [0.35, 0.15, 0.20, 0.10, 0.10, 0.10],
            Preference::Cost => [0.20, 0.05, 0.60, 0.05, 0.05, 0.05],
            Preference::Speed => [0.25, 0.05, 0.15, 0.40, 0.05, 0.10],
        }
    }
}

/// A request to pick a model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RouteRequest {
    /// What the user wants done (may be empty when `activity` is given).
    pub task: String,
    /// `None` = detected from `task`.
    pub activity: Option<Activity>,
    /// `None` = the Council setting, else the activity's default.
    pub preference: Option<Preference>,
    /// `None` = the activity's default.
    pub needs_tools: Option<bool>,
    /// Minimum context window in tokens.
    pub min_context: Option<u32>,
}

/// A model identified by provider + model id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRef {
    pub provider: ProviderId,
    pub model: String,
}

/// Criteria of a score, each from 0 to 1.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Criteria {
    pub tags: f64,
    pub quality: f64,
    pub cost: f64,
    pub speed: f64,
    pub context: f64,
    pub tools: f64,
}

/// A model that can do the task, with its score (0–100) and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    #[serde(flatten)]
    pub model_ref: ModelRef,
    pub provider_name: String,
    pub model_name: String,
    pub score: f64,
    pub criteria: Criteria,
    /// Short reasons in Portuguese, most relevant first.
    pub reasons: Vec<String>,
    pub context_window: Option<u32>,
    pub input_price: Option<f64>,
    pub output_price: Option<f64>,
    pub supports_tools: Option<bool>,
    pub tags: Vec<String>,
}

/// A model left out, and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Excluded {
    #[serde(flatten)]
    pub model_ref: ModelRef,
    pub provider_name: String,
    pub reason: String,
}

/// The router's answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recommendation {
    pub activity: Activity,
    /// True when the activity was detected from the task.
    pub detected: bool,
    pub preference: Preference,
    pub needs_tools: bool,
    pub min_context: Option<u32>,
    /// Best first.
    pub candidates: Vec<Candidate>,
    pub excluded: Vec<Excluded>,
}

impl Recommendation {
    pub fn best(&self) -> Option<&Candidate> {
        self.candidates.first()
    }

    pub fn find(&self, model: &ModelRef) -> Option<&Candidate> {
        self.candidates.iter().find(|c| &c.model_ref == model)
    }
}

/// Tags that signal a strong (slower, pricier) model.
const QUALITY_TAGS: &[&str] = &[
    "raciocinio",
    "reasoning",
    "premium",
    "avancado",
    "advanced",
    "forte",
    "strong",
    "top",
    "melhor",
    "best",
    "inteligente",
    "smart",
];
/// Tags that signal a fast model.
const SPEED_TAGS: &[&str] = &["rapido", "fast", "veloz", "leve", "light", "lite"];
/// Tags that signal a cheap model.
const CHEAP_TAGS: &[&str] = &["barato", "cheap", "economico", "gratis", "free", "local"];
/// Words of model ids that hint at a large model.
const QUALITY_HINTS: &[&str] = &[
    "opus", "ultra", "large", "pro", "max", "reasoner", "thinking",
];
/// Words of model ids that hint at a small, fast model.
const SPEED_HINTS: &[&str] = &[
    "mini", "nano", "flash", "haiku", "lite", "small", "tiny", "instant", "fast", "turbo",
];

struct Signals {
    quality_tag: bool,
    speed_tag: bool,
    cheap_tag: bool,
    quality_hint: bool,
    speed_hint: bool,
}

/// `llama3:70b` → 70.0 (billions of parameters), when the id says it.
fn parameter_size(words: &[String]) -> Option<f64> {
    words.iter().find_map(|w| {
        let digits = w.strip_suffix('b')?;
        let value: f64 = digits.parse().ok()?;
        (value > 0.0 && value < 2000.0).then_some(value)
    })
}

fn signals(model: &CatalogModel, tags: &[String]) -> Signals {
    let has = |list: &[&str]| tags.iter().any(|t| list.contains(&t.as_str()));
    let id_words = words(&model.info.id);
    let hint = |list: &[&str]| id_words.iter().any(|w| list.contains(&w.as_str()));
    let size = parameter_size(&id_words);
    Signals {
        quality_tag: has(QUALITY_TAGS),
        speed_tag: has(SPEED_TAGS),
        cheap_tag: has(CHEAP_TAGS),
        quality_hint: hint(QUALITY_HINTS) || size.is_some_and(|b| b >= 65.0),
        speed_hint: hint(SPEED_HINTS) || size.is_some_and(|b| b <= 14.0),
    }
}

/// USD per million tokens, weighting input 3:1 (prompts are larger).
fn blended_price(model: &CatalogModel) -> Option<f64> {
    match (model.info.input_price, model.info.output_price) {
        (None, None) => None,
        (input, output) => {
            let input = input.or(output).unwrap_or(0.0);
            let output = output.or(Some(input)).unwrap_or(0.0);
            Some((3.0 * input + output) / 4.0)
        }
    }
}

fn format_price(value: f64) -> String {
    if value == 0.0 {
        "0".into()
    } else if value < 1.0 {
        format!("{value:.2}")
    } else {
        let text = format!("{value:.2}");
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

pub fn format_context(tokens: u32) -> String {
    if tokens >= 1_000_000 && tokens.is_multiple_of(1_000_000) {
        format!("{}M", tokens / 1_000_000)
    } else if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

/// Why a model cannot do the task, if it cannot.
fn exclusion(model: &CatalogModel, needs_tools: bool, min_context: Option<u32>) -> Option<String> {
    if model.available == Some(false) {
        return Some(match &model.availability_detail {
            Some(detail) => format!("provider indisponível: {detail}"),
            None => "provider indisponível".into(),
        });
    }
    if needs_tools && !model.provider_tools {
        return Some(
            "a conexão está com as ferramentas desligadas e a atividade precisa delas".into(),
        );
    }
    if needs_tools && model.info.supports_tools == Some(false) {
        return Some("o modelo não chama ferramentas e a atividade precisa delas".into());
    }
    if let (Some(min), Some(context)) = (min_context, model.info.context_window) {
        if context < min {
            return Some(format!(
                "contexto de {} abaixo do mínimo pedido ({})",
                format_context(context),
                format_context(min)
            ));
        }
    }
    None
}

/// Ranks `models` for `request`. `default_preference` applies when the
/// request has none (the Council setting); else the activity's own.
pub fn rank(
    models: &[CatalogModel],
    request: &RouteRequest,
    default_preference: Option<Preference>,
) -> Recommendation {
    let (activity, detected) = match request.activity {
        Some(activity) => (activity, false),
        None => (detect(&request.task), true),
    };
    let profile = activity.profile();
    let preference = request
        .preference
        .or(default_preference)
        .unwrap_or(profile.preference);
    let needs_tools = request.needs_tools.unwrap_or(profile.needs_tools);
    let task_words: BTreeSet<String> = words(&request.task)
        .into_iter()
        .filter(|w| w.len() >= 3)
        .collect();

    let mut excluded = Vec::new();
    let mut eligible = Vec::new();
    for model in models {
        let model_ref = ModelRef {
            provider: model.provider.clone(),
            model: model.info.id.clone(),
        };
        match exclusion(model, needs_tools, request.min_context) {
            Some(reason) => excluded.push(Excluded {
                model_ref,
                provider_name: model.provider_name.clone(),
                reason,
            }),
            None => eligible.push((model_ref, model)),
        }
    }

    // Price range of the eligible models with a known, non-zero price.
    let prices: Vec<f64> = eligible
        .iter()
        .filter_map(|(_, m)| blended_price(m))
        .filter(|p| *p > 0.0)
        .collect();
    let has_free = eligible
        .iter()
        .any(|(_, m)| blended_price(m).is_some_and(|p| p <= 0.0));
    let (min_price, max_price) = prices
        .iter()
        .fold((f64::MAX, 0.0_f64), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    // 1 = free, 0.9 = cheapest paid, 0 = priciest (log scale); `None` =
    // unknown.
    let cheapness = |price: Option<f64>| -> Option<f64> {
        let price = price?;
        if price <= 0.0 {
            return Some(1.0);
        }
        if prices.len() < 2 || max_price <= min_price {
            return Some(0.7);
        }
        let span = max_price.ln() - min_price.ln();
        Some(0.9 * (1.0 - (price.ln() - min_price.ln()) / span))
    };

    let mut candidates: Vec<Candidate> = eligible
        .into_iter()
        .map(|(model_ref, model)| {
            let tags: Vec<String> = model
                .info
                .tags
                .iter()
                .map(|t| normalize(t.trim()))
                .collect();
            let signals = signals(model, &tags);
            let price = blended_price(model);
            let cheap = cheapness(price);
            let mut reasons = Vec::new();

            // Tags that fit the activity or that the task mentions.
            let activity_tags: Vec<&String> = tags
                .iter()
                .filter(|t| profile.tags.contains(&t.as_str()))
                .collect();
            let task_tags: Vec<&String> = tags
                .iter()
                .filter(|t| t.len() >= 3 && task_words.contains(t.as_str()))
                .filter(|t| !activity_tags.contains(t))
                .collect();
            let tag_score = if tags.is_empty() {
                0.25
            } else if activity_tags.is_empty() && task_tags.is_empty() {
                0.1
            } else {
                let activity = match activity_tags.len() {
                    0 => 0.0,
                    1 => 0.7,
                    _ => 1.0,
                };
                (activity + 0.5 * task_tags.len() as f64).min(1.0)
            };
            if !activity_tags.is_empty() {
                reasons.push(format!(
                    "etiquetas da atividade: {}",
                    original_tags(model, &activity_tags)
                ));
            }
            if !task_tags.is_empty() {
                reasons.push(format!(
                    "citado na tarefa: {}",
                    original_tags(model, &task_tags)
                ));
            }

            let expensive = cheap.map(|c| 1.0 - c);
            let mut quality: f64 = 0.4;
            if signals.quality_tag {
                quality += 0.35;
            }
            if signals.quality_hint {
                quality += 0.25;
            }
            if signals.speed_hint || signals.speed_tag {
                quality -= 0.15;
            }
            if let Some(expensive) = expensive {
                quality += 0.1 * expensive;
            }
            let quality = quality.clamp(0.0, 1.0);

            let mut speed: f64 = 0.4;
            if signals.speed_tag {
                speed += 0.35;
            }
            if signals.speed_hint {
                speed += 0.25;
            }
            if signals.quality_hint {
                speed -= 0.15;
            }
            if let Some(cheap) = cheap {
                speed += 0.1 * cheap;
            }
            let speed = speed.clamp(0.0, 1.0);

            let cost = match cheap {
                Some(cheap) => cheap,
                None if signals.cheap_tag => 0.6,
                None => 0.5,
            };
            match (price, cheap) {
                (Some(p), _) if p <= 0.0 => reasons.push("sem custo por token".into()),
                (Some(p), _) if p <= min_price && prices.len() > 1 => reasons.push(if has_free {
                    "o mais barato entre os pagos".into()
                } else {
                    "o mais barato entre os candidatos".into()
                }),
                (Some(_), _) => reasons.push(format!(
                    "US$ {} / {} por M tokens",
                    format_price(model.info.input_price.unwrap_or(0.0)),
                    format_price(model.info.output_price.unwrap_or(0.0))
                )),
                (None, _) => reasons.push("preço não informado na conexão".into()),
            }
            if signals.quality_tag || signals.quality_hint {
                reasons.push("perfil de modelo forte".into());
            } else if signals.speed_tag || signals.speed_hint {
                reasons.push("perfil de modelo rápido".into());
            }

            let context = match model.info.context_window {
                Some(tokens) => {
                    let bits = (tokens.max(1) as f64).log2();
                    ((bits - 12.0) / 5.0).clamp(0.0, 1.0)
                }
                None => 0.5,
            };
            if let Some(tokens) = model.info.context_window {
                reasons.push(format!("contexto {}", format_context(tokens)));
            }

            let tools = match model.info.supports_tools {
                Some(true) => 1.0,
                Some(false) => 0.0,
                None => 0.6,
            };
            if needs_tools {
                reasons.push(match model.info.supports_tools {
                    Some(true) => "chama ferramentas".into(),
                    _ => "suporte a ferramentas não informado".into(),
                });
            }

            let [w_tags, w_quality, w_cost, w_speed, w_context, w_tools] = preference.weights();
            let (w_tags, w_tools) = if needs_tools {
                (w_tags, w_tools)
            } else {
                (w_tags + w_tools, 0.0)
            };
            let mut score = 100.0
                * (w_tags * tag_score
                    + w_quality * quality
                    + w_cost * cost
                    + w_speed * speed
                    + w_context * context
                    + w_tools * tools);
            // Tie-breakers only.
            if model.is_default {
                score += 1.0;
                reasons.push("modelo padrão da conexão".into());
            }
            if model.provider_active {
                score += 0.5;
            }
            Candidate {
                model_ref,
                provider_name: model.provider_name.clone(),
                model_name: model.info.name.clone(),
                score: (score.clamp(0.0, 100.0) * 10.0).round() / 10.0,
                criteria: Criteria {
                    tags: tag_score,
                    quality,
                    cost,
                    speed,
                    context,
                    tools,
                },
                reasons,
                context_window: model.info.context_window,
                input_price: model.info.input_price,
                output_price: model.info.output_price,
                supports_tools: model.info.supports_tools,
                tags: model.info.tags.clone(),
            }
        })
        .collect();
    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.model_ref.cmp(&b.model_ref))
    });

    Recommendation {
        activity,
        detected,
        preference,
        needs_tools,
        min_context: request.min_context,
        candidates,
        excluded,
    }
}

/// The model's own spelling of the normalized tags in `matched`.
fn original_tags(model: &CatalogModel, matched: &[&String]) -> String {
    model
        .info
        .tags
        .iter()
        .filter(|t| matched.iter().any(|m| **m == normalize(t.trim())))
        .map(|t| t.trim().to_owned())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchestrator_providers::ModelInfo;

    fn model(provider: &str, id: &str, tweak: impl FnOnce(&mut CatalogModel)) -> CatalogModel {
        let mut m = CatalogModel {
            provider: ProviderId::from(provider),
            provider_name: provider.to_uppercase(),
            info: ModelInfo {
                id: id.into(),
                name: id.into(),
                ..Default::default()
            },
            is_default: false,
            provider_active: false,
            provider_tools: true,
            completion: true,
            available: Some(true),
            availability_detail: None,
        };
        tweak(&mut m);
        m
    }

    fn priced(input: f64, output: f64, tags: &[&str]) -> impl FnOnce(&mut CatalogModel) {
        let tags: Vec<String> = tags.iter().map(|t| (*t).to_owned()).collect();
        move |m| {
            m.info.input_price = Some(input);
            m.info.output_price = Some(output);
            m.info.tags = tags;
            m.info.supports_tools = Some(true);
            m.info.context_window = Some(200_000);
        }
    }

    fn catalog() -> Vec<CatalogModel> {
        vec![
            model(
                "anthropic",
                "claude-opus",
                priced(15.0, 75.0, &["código", "raciocínio"]),
            ),
            model(
                "anthropic",
                "claude-haiku",
                priced(0.8, 4.0, &["rápido", "barato"]),
            ),
            model(
                "openai",
                "gpt-mini",
                priced(0.15, 0.6, &["barato", "rápido"]),
            ),
            model("openai", "gpt-coder", priced(2.5, 10.0, &["código"])),
            model("local", "llama3:8b", |m| {
                m.info.input_price = Some(0.0);
                m.info.output_price = Some(0.0);
                m.info.supports_tools = Some(false);
                m.info.tags = vec!["local".into()];
            }),
        ]
    }

    fn names(rec: &Recommendation) -> Vec<&str> {
        rec.candidates
            .iter()
            .map(|c| c.model_ref.model.as_str())
            .collect()
    }

    #[test]
    fn coding_prefers_code_models_and_needs_tools() {
        let rec = rank(
            &catalog(),
            &RouteRequest {
                task: "Implemente o endpoint de pagamentos".into(),
                ..Default::default()
            },
            None,
        );
        assert_eq!(rec.activity, Activity::Code);
        assert!(rec.detected);
        assert!(rec.needs_tools);
        assert_eq!(rec.preference, Preference::Balanced);
        let order = names(&rec);
        assert!(
            order.iter().position(|m| *m == "gpt-coder")
                < order.iter().position(|m| *m == "gpt-mini"),
            "{order:?}"
        );
        // The local model cannot call tools.
        assert!(!order.contains(&"llama3:8b"));
        let local = &rec.excluded[0];
        assert_eq!(local.model_ref.model, "llama3:8b");
        assert!(
            local.reason.contains("não chama ferramentas"),
            "{}",
            local.reason
        );
        let coder = rec
            .candidates
            .iter()
            .find(|c| c.model_ref.model == "gpt-coder")
            .unwrap();
        assert!(
            coder
                .reasons
                .iter()
                .any(|r| r == "etiquetas da atividade: código"),
            "{:?}",
            coder.reasons
        );
    }

    #[test]
    fn preference_changes_the_ranking() {
        let base = RouteRequest {
            task: "Planeje a arquitetura do sistema de filas".into(),
            ..Default::default()
        };
        let quality = rank(&catalog(), &base, None);
        assert_eq!(quality.activity, Activity::Planning);
        assert_eq!(quality.preference, Preference::Quality);
        assert_eq!(names(&quality)[0], "claude-opus");

        let cost = rank(
            &catalog(),
            &RouteRequest {
                preference: Some(Preference::Cost),
                ..base.clone()
            },
            None,
        );
        // Planning does not need tools, so the free local model is eligible
        // and wins on cost.
        assert_eq!(names(&cost)[0], "llama3:8b");
        let local = cost.best().unwrap();
        assert!(local.reasons.contains(&"sem custo por token".to_owned()));
        let mini = cost
            .candidates
            .iter()
            .find(|c| c.model_ref.model == "gpt-mini")
            .unwrap();
        assert!(
            mini.reasons
                .contains(&"o mais barato entre os pagos".to_owned()),
            "{:?}",
            mini.reasons
        );

        // The Council's default preference applies when the request has none.
        let speed = rank(&catalog(), &base, Some(Preference::Speed));
        assert_eq!(speed.preference, Preference::Speed);
        assert!(["gpt-mini", "claude-haiku", "llama3:8b"].contains(&names(&speed)[0]));
    }

    #[test]
    fn requirements_exclude_with_reasons() {
        let mut models = catalog();
        models.push(model("down", "x-large", |m| {
            m.available = Some(false);
            m.availability_detail = Some("401 authentication failed".into());
        }));
        models.push(model("notools", "chat-only", |m| m.provider_tools = false));
        models.push(model("small", "tiny-ctx", |m| {
            m.info.context_window = Some(8_000)
        }));
        let rec = rank(
            &models,
            &RouteRequest {
                task: "corrigir bug".into(),
                min_context: Some(32_000),
                ..Default::default()
            },
            None,
        );
        let reason = |model: &str| {
            rec.excluded
                .iter()
                .find(|e| e.model_ref.model == model)
                .map(|e| e.reason.clone())
                .unwrap_or_default()
        };
        assert_eq!(
            reason("x-large"),
            "provider indisponível: 401 authentication failed"
        );
        assert!(reason("chat-only").contains("ferramentas desligadas"));
        assert_eq!(
            reason("tiny-ctx"),
            "contexto de 8k abaixo do mínimo pedido (32k)"
        );
        assert_eq!(rec.activity, Activity::Debug);
    }

    #[test]
    fn unknown_data_is_neutral_and_explained() {
        let models = vec![model("p", "plain", |_| {})];
        let rec = rank(
            &models,
            &RouteRequest {
                activity: Some(Activity::Code),
                ..Default::default()
            },
            None,
        );
        assert!(!rec.detected);
        let only = rec.best().unwrap();
        assert!(only
            .reasons
            .contains(&"preço não informado na conexão".to_owned()));
        assert!(only
            .reasons
            .contains(&"suporte a ferramentas não informado".to_owned()));
        assert!(only.score > 0.0 && only.score < 100.0);
    }

    #[test]
    fn task_words_match_tags_and_parameter_sizes_hint_speed() {
        let models = vec![
            model("a", "generalist", |m| m.info.tags = vec!["geral".into()]),
            model("b", "pythonista", |m| m.info.tags = vec!["Python".into()]),
        ];
        let rec = rank(
            &models,
            &RouteRequest {
                task: "Olá, me ajude com um script python".into(),
                activity: Some(Activity::General),
                ..Default::default()
            },
            None,
        );
        let python = rec
            .find(&ModelRef {
                provider: "b".into(),
                model: "pythonista".into(),
            })
            .unwrap();
        assert!(
            python
                .reasons
                .contains(&"citado na tarefa: Python".to_owned()),
            "{:?}",
            python.reasons
        );

        let small = model("x", "llama3:8b", |_| {});
        let big = model("x", "llama3:70b", |_| {});
        assert!(signals(&small, &[]).speed_hint);
        assert!(signals(&big, &[]).quality_hint);
    }

    #[test]
    fn formats_prices_and_context() {
        assert_eq!(format_price(15.0), "15");
        assert_eq!(format_price(2.5), "2.5");
        assert_eq!(format_price(0.15), "0.15");
        assert_eq!(format_context(200_000), "200k");
        assert_eq!(format_context(1_000_000), "1M");
        assert_eq!(format_context(512), "512");
    }
}
