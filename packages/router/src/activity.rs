//! Activities: what a task is about, detected from its description without
//! spending tokens (ADR-0011).

use crate::score::Preference;
use serde::{Deserialize, Serialize};

/// Kind of work a model is picked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Activity {
    Code,
    Debug,
    Review,
    Tests,
    Planning,
    Docs,
    Summary,
    General,
}

/// What an activity usually needs.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityProfile {
    pub activity: Activity,
    pub label: &'static str,
    /// Model tags that fit the activity (normalized: lowercase, no accents).
    pub tags: &'static [&'static str],
    /// The work edits or reads the project, so the model must call tools.
    pub needs_tools: bool,
    pub preference: Preference,
}

const PROFILES: [ActivityProfile; 8] = [
    ActivityProfile {
        activity: Activity::Code,
        label: "Implementar código",
        tags: &[
            "codigo",
            "code",
            "coding",
            "programacao",
            "programming",
            "dev",
        ],
        needs_tools: true,
        preference: Preference::Balanced,
    },
    ActivityProfile {
        activity: Activity::Debug,
        label: "Depurar um erro",
        tags: &[
            "debug",
            "depuracao",
            "codigo",
            "code",
            "raciocinio",
            "reasoning",
        ],
        needs_tools: true,
        preference: Preference::Quality,
    },
    ActivityProfile {
        activity: Activity::Review,
        label: "Revisar código",
        tags: &[
            "revisao",
            "review",
            "codigo",
            "code",
            "raciocinio",
            "reasoning",
        ],
        needs_tools: true,
        preference: Preference::Quality,
    },
    ActivityProfile {
        activity: Activity::Tests,
        label: "Escrever testes",
        tags: &["testes", "tests", "testing", "codigo", "code"],
        needs_tools: true,
        preference: Preference::Balanced,
    },
    ActivityProfile {
        activity: Activity::Planning,
        label: "Planejar / arquitetura",
        tags: &[
            "planejamento",
            "planning",
            "arquitetura",
            "architecture",
            "raciocinio",
            "reasoning",
        ],
        needs_tools: false,
        preference: Preference::Quality,
    },
    ActivityProfile {
        activity: Activity::Docs,
        label: "Documentação / escrita",
        tags: &[
            "escrita",
            "writing",
            "docs",
            "documentacao",
            "texto",
            "text",
        ],
        needs_tools: true,
        preference: Preference::Cost,
    },
    ActivityProfile {
        activity: Activity::Summary,
        label: "Resumo / pergunta rápida",
        tags: &[
            "resumo", "summary", "rapido", "fast", "barato", "cheap", "chat",
        ],
        needs_tools: false,
        preference: Preference::Speed,
    },
    ActivityProfile {
        activity: Activity::General,
        label: "Geral",
        tags: &["geral", "general", "chat"],
        needs_tools: false,
        preference: Preference::Balanced,
    },
];

impl Activity {
    pub const ALL: [Activity; 8] = [
        Activity::Code,
        Activity::Debug,
        Activity::Review,
        Activity::Tests,
        Activity::Planning,
        Activity::Docs,
        Activity::Summary,
        Activity::General,
    ];

    pub fn profile(self) -> &'static ActivityProfile {
        PROFILES
            .iter()
            .find(|p| p.activity == self)
            .expect("every activity has a profile")
    }

    pub fn label(self) -> &'static str {
        self.profile().label
    }

    /// Stable id, as serialized.
    pub fn id(self) -> &'static str {
        match self {
            Activity::Code => "code",
            Activity::Debug => "debug",
            Activity::Review => "review",
            Activity::Tests => "tests",
            Activity::Planning => "planning",
            Activity::Docs => "docs",
            Activity::Summary => "summary",
            Activity::General => "general",
        }
    }
}

/// Every activity profile, in display order.
pub fn profiles() -> &'static [ActivityProfile] {
    &PROFILES
}

/// Keywords (normalized word prefixes, or phrases with a space) that point
/// to an activity. The first word (usually the verb: "Resuma o README")
/// counts twice; earlier activities win ties (a review of a bug fix is a
/// review).
const KEYWORDS: [(Activity, &[&str]); 7] = [
    (
        Activity::Review,
        &[
            "revis",
            "review",
            "avali",
            "audit",
            "pull request",
            "code review",
        ],
    ),
    (
        Activity::Debug,
        &[
            "bug",
            "erro",
            "error",
            "falha",
            "falhando",
            "crash",
            "debug",
            "depur",
            "corrig",
            "consert",
            "fix",
            "quebr",
            "exception",
            "excecao",
            "stack trace",
            "nao funciona",
            "not working",
            "broken",
        ],
    ),
    (
        Activity::Tests,
        &[
            "teste",
            "test",
            "cobertura",
            "coverage",
            "unitari",
            "e2e",
            "vitest",
            "pytest",
            "jest",
        ],
    ),
    (
        Activity::Planning,
        &[
            "planej",
            "plano",
            "plan",
            "arquitet",
            "architect",
            "design",
            "proposta",
            "propose",
            "estrategi",
            "strategy",
            "roadmap",
            "adr",
        ],
    ),
    (
        Activity::Docs,
        &[
            "document",
            "readme",
            "redij",
            "redig",
            "changelog",
            "tutorial",
            "guia",
            "docs",
            "artigo",
        ],
    ),
    (
        Activity::Summary,
        &[
            "resum",
            "explique",
            "explica",
            "o que e",
            "o que sao",
            "como funciona",
            "summar",
            "explain",
            "what is",
            "tldr",
            "traduz",
            "translat",
            "pergunta",
            "duvida",
        ],
    ),
    (
        Activity::Code,
        &[
            "implement",
            "criar",
            "crie",
            "cria",
            "adicion",
            "adicione",
            "refator",
            "refactor",
            "codigo",
            "code",
            "funcao",
            "function",
            "classe",
            "class",
            "endpoint",
            "feature",
            "funcionalidade",
            "component",
            "migr",
            "integr",
            "create",
            "add",
            "build",
            "programa",
            "script",
        ],
    ),
];

/// Lowercase without accents: "Revisão" → "revisao".
pub fn normalize(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            other => other,
        })
        .collect()
}

/// Normalized words of a text (letters and digits).
pub fn words(text: &str) -> Vec<String> {
    normalize(text)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The activity a task description points to; `General` when nothing
/// matches.
pub fn detect(task: &str) -> Activity {
    let words = words(task);
    let phrase = format!(" {} ", words.join(" "));
    let first = words.first().map(String::as_str).unwrap_or("");
    let mut best = (Activity::General, 0usize);
    for (activity, keywords) in KEYWORDS {
        let mut hits = keywords
            .iter()
            .filter(|keyword| {
                if keyword.contains(' ') {
                    phrase.contains(&format!(" {keyword} "))
                } else {
                    words.iter().any(|w| w.starts_with(*keyword))
                }
            })
            .count();
        if keywords
            .iter()
            .any(|k| !k.contains(' ') && first.starts_with(*k))
        {
            hits += 1;
        }
        if hits > best.1 {
            best = (activity, hits);
        }
    }
    best.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_accents_and_case() {
        assert_eq!(normalize("Revisão de Código"), "revisao de codigo");
        assert_eq!(
            words("Corrigir o erro: 'não funciona'!"),
            ["corrigir", "o", "erro", "nao", "funciona"]
        );
    }

    #[test]
    fn detects_the_activity_of_a_task() {
        let cases = [
            (
                "Implemente um endpoint de pagamentos com Stripe",
                Activity::Code,
            ),
            (
                "Corrigir o erro de login que está quebrando o app",
                Activity::Debug,
            ),
            ("Revise o código do PR de autenticação", Activity::Review),
            ("Escreva testes unitários para o parser", Activity::Tests),
            (
                "Planeje a arquitetura do módulo de filas",
                Activity::Planning,
            ),
            ("Atualize o README e a documentação da API", Activity::Docs),
            ("Resuma o que esse projeto faz", Activity::Summary),
            ("Resuma o README em três frases", Activity::Summary),
            ("Escreva testes para o parser", Activity::Tests),
            ("Escreva a documentação do módulo", Activity::Docs),
            ("Fix the crash when the list is empty", Activity::Debug),
            ("Explain what this repository does", Activity::Summary),
            ("olá", Activity::General),
            ("", Activity::General),
        ];
        for (task, expected) in cases {
            assert_eq!(detect(task), expected, "{task}");
        }
    }

    #[test]
    fn every_activity_has_a_profile_with_normalized_tags() {
        for activity in Activity::ALL {
            let profile = activity.profile();
            assert_eq!(profile.activity, activity);
            for tag in profile.tags {
                assert_eq!(normalize(tag), *tag);
            }
            let id = serde_json::to_value(activity).unwrap();
            assert_eq!(id, activity.id());
        }
    }
}
