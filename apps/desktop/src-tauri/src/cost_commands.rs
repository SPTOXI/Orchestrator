//! Tauri commands of tokens and cost (ADR-0018): what the AIs spent, the
//! daily budget of agents and "Compactar".

use crate::AppState;
use chrono::{Duration, Local, Utc};
use orchestrator_agents::BudgetView;
use orchestrator_core::{CallOrigin, SessionId};
use orchestrator_memory::SpendReport;
use orchestrator_providers::{ProviderError, TurnResult};
use tauri::State;

/// Local midnight `days - 1` days ago: 1 = today.
fn since(days: u32) -> chrono::DateTime<Utc> {
    let back = i64::from(days.clamp(1, 366)) - 1;
    let day = Local::now().date_naive() - Duration::days(back);
    day.and_hms_opt(0, 0, 0)
        .and_then(|midnight| midnight.and_local_timezone(Local).earliest())
        .map(|midnight| midnight.with_timezone(&Utc))
        .unwrap_or_else(|| Utc::now() - Duration::days(back + 1))
}

/// What the AIs spent in the last `days` days (1 = today), in the open
/// project or, with `allProjects`, in every project.
#[tauri::command]
pub fn spend_report(
    state: State<'_, AppState>,
    days: u32,
    project_id: Option<String>,
    all_projects: Option<bool>,
) -> SpendReport {
    let project = match all_projects {
        Some(true) => None,
        _ => project_id.or_else(|| state.store.current_project().map(|p| p.id)),
    };
    state.store.spend(project.as_deref(), since(days))
}

/// The open project's spending today against the agents' daily budget.
#[tauri::command]
pub fn agents_budget(state: State<'_, AppState>, project_id: Option<String>) -> BudgetView {
    match project_id.or_else(|| state.store.current_project().map(|p| p.id)) {
        Some(project) => state.agents.budget(&project),
        None => BudgetView {
            spent_today_usd: 0.0,
            budget_usd: state.agents.settings().daily_budget_usd,
            unpriced: 0,
            exhausted: false,
        },
    }
}

/// "Compactar": the AI summarizes the conversation and the summary
/// replaces it (ADR-0018). Waits for the summary.
#[tauri::command]
pub async fn session_compact(
    state: State<'_, AppState>,
    id: String,
) -> Result<TurnResult, ProviderError> {
    state
        .sessions
        .compact(&SessionId::from(id), CallOrigin::User)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_start_at_local_midnight() {
        let today = since(1);
        assert!(today <= Utc::now());
        assert!(Utc::now() - today <= Duration::hours(25));
        let week = since(7);
        let gap = today - week;
        assert!(gap >= Duration::hours(6 * 24 - 1) && gap <= Duration::hours(6 * 24 + 1));
    }
}
