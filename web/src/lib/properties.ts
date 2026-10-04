import type { PropertyFilter } from "../types/PropertyFilter";
import type { PropertyOperator } from "../types/PropertyOperator";

const FRIENDLY: Record<string, string> = {
  $current_url: "Current URL",
  $pathname: "Path name",
  $host: "Host",
  $browser: "Browser",
  $browser_version: "Browser version",
  $os: "OS",
  $os_version: "OS version",
  $device_type: "Device type",
  $screen_width: "Screen width",
  $screen_height: "Screen height",
  $referrer: "Referrer URL",
  $referring_domain: "Referring domain",
  $geoip_country_code: "Country code",
  $geoip_country_name: "Country",
  $geoip_city_name: "City",
  $session_id: "Session ID",
  $lib: "Library",
  $lib_version: "Library version",
  $initial_referring_domain: "Initial referring domain",
  $initial_utm_source: "Initial UTM source",
  $ai_model: "AI model",
  $ai_provider: "AI provider",
  $ai_total_cost_usd: "AI cost (USD)",
  $ai_latency: "AI latency",
  $exception_type: "Exception type",
  utm_source: "UTM source",
  utm_medium: "UTM medium",
  utm_campaign: "UTM campaign",
  utm_content: "UTM content",
  utm_term: "UTM term",
  email: "Email",
  name: "Name",
};

const EVENT_FRIENDLY: Record<string, string> = {
  $pageview: "Pageview",
  $pageleave: "Pageleave",
  $autocapture: "Autocapture",
  $identify: "Identify",
  $create_alias: "Alias",
  $set: "Set person properties",
  $groupidentify: "Group identify",
  $feature_flag_called: "Feature flag called",
  $exception: "Exception",
  $ai_generation: "AI generation",
  $ai_span: "AI span",
  $ai_trace: "AI trace",
  $ai_embedding: "AI embedding",
  $rageclick: "Rageclick",
  $web_vitals: "Web vitals",
  $survey_shown: "Survey shown",
};

export function propertyLabel(key: string): string {
  return FRIENDLY[key] ?? key;
}

export function eventLabel(name: string | null | undefined): string {
  if (name === null || name === undefined) return "All events";
  return EVENT_FRIENDLY[name] ?? name;
}

export const OPERATORS: { value: PropertyOperator; label: string; short: string; needsValue: boolean; numeric?: boolean; multi?: boolean }[] = [
  { value: "exact", label: "equals", short: "=", needsValue: true, multi: true },
  { value: "is_not", label: "doesn't equal", short: "≠", needsValue: true, multi: true },
  { value: "icontains", label: "contains", short: "∋", needsValue: true },
  { value: "not_icontains", label: "doesn't contain", short: "∌", needsValue: true },
  { value: "regex", label: "matches regex", short: "~", needsValue: true },
  { value: "not_regex", label: "doesn't match regex", short: "!~", needsValue: true },
  { value: "gt", label: "greater than", short: ">", needsValue: true, numeric: true },
  { value: "gte", label: "at least", short: "≥", needsValue: true, numeric: true },
  { value: "lt", label: "less than", short: "<", needsValue: true, numeric: true },
  { value: "lte", label: "at most", short: "≤", needsValue: true, numeric: true },
  { value: "is_set", label: "is set", short: "is set", needsValue: false },
  { value: "is_not_set", label: "is not set", short: "is not set", needsValue: false },
  { value: "is_date_before", label: "is before", short: "before", needsValue: true },
  { value: "is_date_after", label: "is after", short: "after", needsValue: true },
];

export function operatorInfo(op: PropertyOperator) {
  return OPERATORS.find((o) => o.value === op) ?? OPERATORS[0];
}

export function filterValueText(value: PropertyFilter["value"]): string {
  if (value === null || value === undefined) return "";
  if (Array.isArray(value)) return value.map(String).join(" or ");
  return String(value);
}

export function describeFilter(f: PropertyFilter): { key: string; op: string; value: string } {
  const info = operatorInfo(f.operator);
  return { key: propertyLabel(f.key), op: info.label, value: info.needsValue ? filterValueText(f.value) : "" };
}

/** Drop half-built filters before a query is sent. */
export function completeFilters(filters: PropertyFilter[]): PropertyFilter[] {
  return filters.filter((f) => {
    if (!f.key) return false;
    const info = operatorInfo(f.operator);
    if (!info.needsValue) return true;
    if (Array.isArray(f.value)) return f.value.length > 0;
    return f.value !== null && f.value !== "";
  });
}
