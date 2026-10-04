// Live event feed: newest first, polled, expandable to the full payload.

import { Fragment, useEffect, useRef, useState } from "react";
import type { EventRow } from "../types/EventRow";
import { api, errorMessage } from "../lib/api";
import { usePath, useProjectId } from "../lib/context";
import { fmtDateTime, fmtRelative } from "../lib/format";
import { useApi, useNow } from "../lib/hooks";
import { eventLabel } from "../lib/properties";
import { Link, navigate, useLocation } from "../lib/router";
import { Icon } from "../ui/icons";
import { CopyButton, Empty, ErrorState, JsonView, Seg, SkeletonRows, Switch } from "../ui/kit";
import { EventPicker } from "../insight/pickers";
import { LoadDemoButton } from "./Onboarding";

function str(v: unknown): string {
  if (v === null || v === undefined) return "";
  return typeof v === "object" ? JSON.stringify(v) : String(v);
}

export function EventDetail({ event }: { event: EventRow }) {
  const [mode, setMode] = useState<"table" | "json">("table");
  const props = event.properties ?? {};
  const keys = Object.keys(props).sort((a, b) => Number(a.startsWith("$")) - Number(b.startsWith("$")) || a.localeCompare(b));
  return (
    <div className="col gap-12" style={{ padding: "8px 4px 12px" }}>
      <div className="row wrap">
        <Seg
          label="View"
          value={mode}
          onChange={setMode}
          options={[
            { value: "table", label: "Properties" },
            { value: "json", label: "JSON" },
          ]}
        />
        <span className="spacer" />
        <span className="muted small mono">{event.uuid}</span>
        <CopyButton text={JSON.stringify(event, null, 2)} label="Copy event" />
      </div>
      {mode === "json" ? (
        <JsonView value={event} />
      ) : (
        <div className="kv">
          <div>event</div>
          <div>{event.event}</div>
          <div>distinct_id</div>
          <div>{event.distinct_id}</div>
          <div>timestamp</div>
          <div>{event.timestamp}</div>
          {keys.map((k) => (
            <Fragment key={k}>
              <div>{k}</div>
              <div>{str(props[k]) || <span className="muted">""</span>}</div>
            </Fragment>
          ))}
        </div>
      )}
    </div>
  );
}

export function EventTable({ events, showPerson = true, fresh }: { events: EventRow[]; showPerson?: boolean; fresh?: Set<string> }) {
  const path = usePath();
  const [open, setOpen] = useState<string | null>(null);
  const now = useNow(5000);
  return (
    <table className="table">
      <thead>
        <tr>
          <th style={{ width: 28 }} />
          <th>Event</th>
          {showPerson && <th>Person</th>}
          <th>URL / screen</th>
          <th>Library</th>
          <th className="r">Time</th>
        </tr>
      </thead>
      <tbody>
        {events.map((e) => {
          const p = e.properties ?? {};
          const url = str(p.$pathname) || str(p.$current_url) || str(p.$screen_name);
          const expanded = open === e.uuid;
          return (
            <Fragment key={e.uuid}>
              <tr
                className={`clickable feed-row${expanded ? " expanded" : ""}${fresh?.has(e.uuid) ? " fresh-row" : ""}`}
                onClick={() => setOpen(expanded ? null : e.uuid)}
                aria-expanded={expanded}
              >
                <td>
                  <Icon name={expanded ? "chevronDown" : "chevronRight"} size={13} style={{ color: "var(--ink-3)" }} />
                </td>
                <td>
                  <span className="event-name">
                    {eventLabel(e.event)}
                    {eventLabel(e.event) !== e.event && <span className="sys mono small">{e.event}</span>}
                  </span>
                </td>
                {showPerson && (
                  <td className="truncate" style={{ maxWidth: 220 }}>
                    <Link to={path(`persons/${encodeURIComponent(e.person_id)}`)} className="link mono small" onClick={(ev) => ev.stopPropagation()}>
                      {e.distinct_id}
                    </Link>
                  </td>
                )}
                <td className="truncate secondary" style={{ maxWidth: 320 }} title={url}>
                  {url || <span className="muted">–</span>}
                </td>
                <td className="muted small nowrap">{str(p.$lib) || "–"}</td>
                <td className="r muted nowrap" title={fmtDateTime(e.timestamp)}>
                  {fmtRelative(e.timestamp, now)}
                </td>
              </tr>
              {expanded && (
                <tr>
                  <td />
                  <td colSpan={showPerson ? 5 : 4} style={{ background: "var(--surface)" }}>
                    <EventDetail event={e} />
                  </td>
                </tr>
              )}
            </Fragment>
          );
        })}
      </tbody>
    </table>
  );
}

export function ActivityPage() {
  const projectId = useProjectId();
  const path = usePath();
  const loc = useLocation();
  const [event, setEvent] = useState<string | null>(loc.search.get("event"));
  const [personId, setPersonId] = useState(loc.search.get("person_id") ?? "");
  const [live, setLive] = useState(true);
  const [older, setOlder] = useState<EventRow[]>([]);
  const [olderCursor, setOlderCursor] = useState<string | null | undefined>(undefined);
  const [olderError, setOlderError] = useState<string | null>(null);
  const [fresh, setFresh] = useState<Set<string>>(new Set());
  const seen = useRef<Set<string> | null>(null);

  const key = `events:${projectId}:${event ?? ""}:${personId}`;
  const { data, error, loading, reload } = useApi(key, (s) => api.events(projectId, { event, person_id: personId || null, limit: 100 }, s), { pollMs: live ? 5000 : undefined });

  useEffect(() => {
    setOlder([]);
    setOlderCursor(undefined);
    seen.current = null;
  }, [key]);

  // Highlight rows that arrived since the last poll.
  useEffect(() => {
    if (!data) return;
    const ids = data.events.map((e) => e.uuid);
    if (seen.current) {
      const added = ids.filter((id) => !seen.current!.has(id));
      if (added.length) {
        setFresh(new Set(added));
        const t = window.setTimeout(() => setFresh(new Set()), 2200);
        for (const id of ids) seen.current.add(id);
        return () => window.clearTimeout(t);
      }
    } else seen.current = new Set(ids);
    return undefined;
  }, [data]);

  const syncUrl = (ev: string | null, pid: string) => {
    const q = new URLSearchParams();
    if (ev) q.set("event", ev);
    if (pid) q.set("person_id", pid);
    navigate(`${path("activity")}${q.toString() ? `?${q}` : ""}`, { replace: true });
  };

  const events = [...(data?.events ?? []), ...older.filter((o) => !data?.events.some((e) => e.uuid === o.uuid))];
  const cursor = olderCursor === undefined ? data?.next_before : olderCursor;

  return (
    <div className="page">
      <div className="page-head">
        <div className="titles">
          <h1>Activity</h1>
          <div className="sub">Every event as it lands, newest first.</div>
        </div>
        <div className="actions">
          <label className="row small secondary" style={{ gap: 8 }}>
            <Switch checked={live} onChange={setLive} label="Live updates" />
            {live ? (
              <span className="row gap-4">
                <span className="dot live" /> Live
              </span>
            ) : (
              "Paused"
            )}
          </label>
          <button className="btn icon" onClick={reload} aria-label="Refresh" title="Refresh">
            <Icon name="refresh" />
          </button>
        </div>
      </div>

      <div className="toolbar">
        <div style={{ width: 260 }} className="row">
          <EventPicker
            value={event}
            onChange={(e) => {
              setEvent(e);
              syncUrl(e, personId);
            }}
          />
        </div>
        <div className="search" style={{ width: 280 }}>
          <Icon name="person" size={14} />
          <input
            className="input"
            placeholder="Person ID"
            value={personId}
            onChange={(e) => setPersonId(e.target.value.trim())}
            onBlur={() => syncUrl(event, personId)}
            aria-label="Filter by person ID"
          />
        </div>
        {(event || personId) && (
          <button
            className="btn ghost small"
            onClick={() => {
              setEvent(null);
              setPersonId("");
              syncUrl(null, "");
            }}
          >
            Clear
          </button>
        )}
      </div>

      <div className="card">
        {error && !data ? (
          <ErrorState error={error} retry={reload} />
        ) : !data && loading ? (
          <SkeletonRows rows={10} />
        ) : events.length === 0 ? (
          event || personId ? (
            <Empty icon="search" title="No events match these filters" />
          ) : (
            <Empty
              icon="activity"
              title="No events yet"
              action={
                <div className="row">
                  <LoadDemoButton />
                  <Link className="btn primary" to={path("onboarding")}>
                    Connect your app
                  </Link>
                </div>
              }
            >
              Point a PostHog SDK at this server and events show up here within seconds. This page refreshes on its own.
            </Empty>
          )
        ) : (
          <div className="table-wrap">
            <EventTable events={events} fresh={fresh} />
          </div>
        )}
      </div>
      {cursor && events.length > 0 && (
        <div className="row mt-16" style={{ justifyContent: "center" }}>
          <button
            className="btn"
            onClick={async () => {
              setOlderError(null);
              try {
                const r = await api.events(projectId, { event, person_id: personId || null, before: cursor, limit: 100 });
                setOlder((o) => [...o, ...r.events]);
                setOlderCursor(r.next_before);
              } catch (e) {
                setOlderError(errorMessage(e));
              }
            }}
          >
            Load older events
          </button>
          {olderError && <span className="small" style={{ color: "var(--bad)" }}>{olderError}</span>}
        </div>
      )}
    </div>
  );
}
