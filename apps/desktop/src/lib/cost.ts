// Pure helpers of the "Tokens e custo" tab (ADR-0018).

export const SPEND_PERIODS: Array<{ days: number; label: string }> = [
  { days: 1, label: "Hoje" },
  { days: 7, label: "7 dias" },
  { days: 30, label: "30 dias" },
];

export function spendLabel(days: number): string {
  if (days <= 1) return "Hoje (desde a meia-noite)";
  return `Últimos ${days} dias`;
}

/** Share of the input read from the cache, 0–100 (null without input). */
export function cacheRatio(cached: number, input: number): number | null {
  if (!input) return null;
  return Math.round((Math.min(cached, input) / input) * 100);
}
