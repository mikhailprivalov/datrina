// W34: encode/decode dashboard parameter selections into window.location.hash
// so reload and local share inside the same profile restore the chosen
// values. Scoped per dashboard via the `d=` segment — switching dashboards
// drops stale entries from another dashboard's hash automatically.
//
// Encoding format (after the existing route fragment):
//   #/<route>?d=<dashboard_id>&p.<name>=<json>
// or, on the default dashboards route:
//   #?d=<dashboard_id>&p.<name>=<json>
//
// Values are JSON-encoded so non-string parameter values (numbers, bools,
// arrays, time ranges) round-trip exactly. Strings still encode as JSON
// strings; the value parser falls back to the raw token if a value is
// not parseable JSON so old hash links don't blow up.

import type { ParameterValue } from './api';

interface HashState {
  route: string;
  dashboardId?: string;
  params: Record<string, string>;
  extra: URLSearchParams;
}

function readHash(): HashState {
  if (typeof window === 'undefined') {
    return { route: '', params: {}, extra: new URLSearchParams() };
  }
  const raw = window.location.hash.startsWith('#') ? window.location.hash.slice(1) : window.location.hash;
  const [route, query = ''] = raw.split('?');
  const search = new URLSearchParams(query);
  const params: Record<string, string> = {};
  const extra = new URLSearchParams();
  let dashboardId: string | undefined;
  for (const [key, value] of search.entries()) {
    if (key === 'd') {
      dashboardId = value;
    } else if (key.startsWith('p.')) {
      params[key.slice(2)] = value;
    } else {
      extra.append(key, value);
    }
  }
  return { route, dashboardId, params, extra };
}

function writeHash(state: HashState) {
  if (typeof window === 'undefined') return;
  const search = new URLSearchParams();
  if (state.dashboardId) search.set('d', state.dashboardId);
  for (const [name, value] of Object.entries(state.params)) {
    search.set(`p.${name}`, value);
  }
  for (const [key, value] of state.extra.entries()) {
    search.append(key, value);
  }
  const query = search.toString();
  const next = query ? `${state.route}?${query}` : state.route;
  const target = next ? `#${next}` : '';
  // Avoid pushing duplicate history entries: only update when the hash
  // actually changes. `history.replaceState` keeps the hash without
  // emitting a `hashchange` event, which would otherwise re-render the
  // route reducer needlessly.
  const current = typeof window !== 'undefined' ? window.location.hash : '';
  if (current !== target) {
    history.replaceState(null, '', `${window.location.pathname}${window.location.search}${target}`);
  }
}

/** Read parameter selections from the URL hash for `dashboardId`. */
export function readParameterHash(dashboardId: string): Record<string, ParameterValue> | null {
  const state = readHash();
  if (state.dashboardId !== dashboardId) return null;
  if (Object.keys(state.params).length === 0) return null;
  const out: Record<string, ParameterValue> = {};
  for (const [name, raw] of Object.entries(state.params)) {
    out[name] = decodeValue(raw);
  }
  return out;
}

/** Replace parameter selections in the URL hash for `dashboardId`. */
export function writeParameterHash(
  dashboardId: string,
  values: Record<string, ParameterValue>,
) {
  const state = readHash();
  // If the dashboard id in the hash is different (or absent), reset the
  // params slot to the new dashboard. Otherwise merge.
  if (state.dashboardId !== dashboardId) {
    state.params = {};
  }
  state.dashboardId = dashboardId;
  state.params = {};
  for (const [name, value] of Object.entries(values)) {
    if (value === undefined || value === null) continue;
    state.params[name] = encodeValue(value);
  }
  writeHash(state);
}

/** Drop the dashboard parameter slot from the hash entirely. */
export function clearParameterHash() {
  const state = readHash();
  if (state.dashboardId === undefined && Object.keys(state.params).length === 0) return;
  state.dashboardId = undefined;
  state.params = {};
  writeHash(state);
}

function encodeValue(value: ParameterValue): string {
  return JSON.stringify(value);
}

function decodeValue(raw: string): ParameterValue {
  try {
    const parsed = JSON.parse(raw);
    if (parsed === null) return raw;
    return parsed as ParameterValue;
  } catch {
    return raw;
  }
}
