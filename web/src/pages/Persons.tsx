import { Fragment, useState } from "react";
import type { EventRow } from "../types/EventRow";
import type { PersonSummary } from "../types/PersonSummary";
import { api, errorMessage } from "../lib/api";
import { canEdit, useApp, usePath, useProjectId } from "../lib/context";
import { fmtDateTime, fmtNumber, fmtRelative } from "../lib/format";
import { invalidate, useApi, useDebounced } from "../lib/hooks";
import { Link, navigate } from "../lib/router";
import { Icon } from "../ui/icons";
import { Avatar, CopyButton, Empty, ErrorState, Modal, Skeleton, SkeletonRows, Tabs, toast } from "../ui/kit";
import { EventTable } from "./Activity";
import { LoadDemoButton } from "./Onboarding";

export function PersonsPage() {
  const projectId = useProjectId();
  const path = usePath();
  const [search, setSearch] = useState("");
  const q = useDebounced(search.trim(), 250);
  const { data, error, loading, reload } = useApi(`persons:${projectId}:${q}`, (s) => api.persons(projectId, { search: q, limit: 50 }, s));
  const [more, setMore] = useState<{ q: string; persons: PersonSummary[]; cursor: string | null } | null>(null);
  const [moreError, setMoreError] = useState<string | null>(null);
  const extra = more && more.q === q ? more : null;
  const persons = [...(data?.persons ?? []), ...(extra?.persons ?? [])];
  const cursor = extra ? extra.cursor : data?.next_cursor ?? null;

  return (
    <div className="page">
      <div className="page-head">
        <div className="titles">
          <h1>Persons</h1>
          <div className="sub">Everyone who sent an event, merged across devices by identify and alias.</div>
        </div>
      </div>
      <div className="card">
        <div className="card-head">
          <div className="search" style={{ width: 360 }}>
            <Icon name="search" size={14} />
            <input
              className="input"
              placeholder="Search by email, name or distinct ID…"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              aria-label="Search persons"
              autoFocus
            />
          </div>
          <span className="spacer" />
          {loading && data && <span className="muted small">Searching…</span>}
        </div>
        {error && !data ? (
          <ErrorState error={error} retry={reload} />
        ) : !data ? (
          <SkeletonRows rows={10} />
        ) : persons.length === 0 ? (
          q ? (
            <Empty icon="search" title={`No one matches “${q}”`} />
          ) : (
            <Empty icon="users" title="No persons yet" action={<LoadDemoButton />}>
              Persons appear when events arrive. Call <code>posthog.identify(userId, {"{ email }"})</code> after login to link anonymous visits to a known user.
            </Empty>
          )
        ) : (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>Person</th>
                  <th>Distinct ID</th>
                  <th>Created</th>
                  <th className="r">Last seen</th>
                </tr>
              </thead>
              <tbody>
                {persons.map((p) => (
                  <tr key={p.id} className="clickable" onClick={() => navigate(path(`persons/${encodeURIComponent(p.id)}`))}>
                    <td>
                      <div className="row">
                        <Avatar name={p.display_name} id={p.id} />
                        <Link to={path(`persons/${encodeURIComponent(p.id)}`)} onClick={(e) => e.stopPropagation()} className="truncate" style={{ fontWeight: 600, maxWidth: 300 }}>
                          {p.display_name}
                        </Link>
                        {p.is_identified ? <span className="badge accent">identified</span> : <span className="badge">anonymous</span>}
                      </div>
                    </td>
                    <td className="mono small muted truncate" style={{ maxWidth: 260 }}>
                      {p.distinct_ids[0]}
                      {p.distinct_ids.length > 1 && <span className="badge" style={{ marginLeft: 6 }}>+{p.distinct_ids.length - 1}</span>}
                    </td>
                    <td className="muted nowrap">{fmtRelative(p.created_at)}</td>
                    <td className="r muted nowrap">{p.last_seen ? fmtRelative(p.last_seen) : "–"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
      {cursor && persons.length > 0 && (
        <div className="row mt-16" style={{ justifyContent: "center" }}>
          <button
            className="btn"
            onClick={async () => {
              setMoreError(null);
              try {
                const r = await api.persons(projectId, { search: q, cursor, limit: 50 });
                setMore({ q, persons: [...(extra?.persons ?? []), ...r.persons], cursor: r.next_cursor });
              } catch (e) {
                setMoreError(errorMessage(e));
              }
            }}
          >
            Load more
          </button>
          {moreError && <span className="small" style={{ color: "var(--bad)" }}>{moreError}</span>}
        </div>
      )}
    </div>
  );
}

function str(v: unknown): string {
  if (v === null || v === undefined) return "";
  return typeof v === "object" ? JSON.stringify(v) : String(v);
}

function PersonEvents({ personId }: { personId: string }) {
  const projectId = useProjectId();
  const { data, error, reload } = useApi(`person-events:${projectId}:${personId}`, (s) => api.personEvents(projectId, personId, { limit: 100 }, s), { pollMs: 15_000 });
  const [older, setOlder] = useState<EventRow[]>([]);
  const [cursor, setCursor] = useState<string | null | undefined>(undefined);
  if (error && !data) return <ErrorState error={error} retry={reload} />;
  if (!data) return <SkeletonRows rows={8} />;
  const events = [...data.events, ...older];
  const next = cursor === undefined ? data.next_before : cursor;
  if (events.length === 0) return <Empty icon="activity" title="No events for this person in the stored range" />;
  return (
    <>
      <div className="table-wrap">
        <EventTable events={events} showPerson={false} />
      </div>
      {next && (
        <div className="row" style={{ justifyContent: "center", padding: 12 }}>
          <button
            className="btn"
            onClick={async () => {
              const r = await api.personEvents(projectId, personId, { before: next, limit: 100 });
              setOlder((o) => [...o, ...r.events]);
              setCursor(r.next_before);
            }}
          >
            Load older
          </button>
        </div>
      )}
    </>
  );
}

function ErasePerson({ personId, name, onClose }: { personId: string; name: string; onClose: () => void }) {
  const projectId = useProjectId();
  const path = usePath();
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ok = typed.trim() === name.trim();
  return (
    <Modal
      title="Delete person and all their data"
      onClose={onClose}
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button
            className="btn danger solid"
            disabled={!ok || busy}
            onClick={async () => {
              setBusy(true);
              setError(null);
              try {
                const r = await api.erasePerson(projectId, personId);
                invalidate(`person`);
                invalidate(`persons:${projectId}`);
                invalidate(`events:${projectId}`);
                toast(`Erased ${fmtNumber(r.events)} events and ${fmtNumber(r.distinct_ids)} distinct IDs`);
                onClose();
                navigate(path("persons"));
              } catch (e) {
                setError(errorMessage(e));
                setBusy(false);
              }
            }}
          >
            {busy ? "Erasing…" : "Erase permanently"}
          </button>
        </>
      }
    >
      <div className="col gap-12">
        <div className="notice bad">
          <Icon name="alert" />
          <div>
            This erases <b>{name}</b>, every distinct ID merged into them, and every event they sent, from stored data. It can't be undone. Use it for
            GDPR and similar deletion requests.
          </div>
        </div>
        <label className="field">
          <span>
            Type <code>{name}</code> to confirm
          </span>
          <input className="input" value={typed} onChange={(e) => setTyped(e.target.value)} autoComplete="off" spellCheck={false} aria-label="Type the person's name to confirm" />
        </label>
        {error && <div className="notice bad">{error}</div>}
      </div>
    </Modal>
  );
}

export function PersonPage({ id }: { id: string }) {
  const projectId = useProjectId();
  const path = usePath();
  const { data, error, reload } = useApi(`person:${projectId}:${id}`, (s) => api.person(projectId, id, s), { keepPrevious: false });
  const [tab, setTab] = useState<"events" | "properties" | "ids">("events");
  const [erasing, setErasing] = useState(false);
  const { organization } = useApp();
  const [propSearch, setPropSearch] = useState("");

  if (error) {
    return (
      <div className="page">
        <ErrorState error={error} retry={reload} />
      </div>
    );
  }
  const p = data?.person;
  const props = (p?.properties ?? {}) as Record<string, unknown>;
  const keys = Object.keys(props)
    .filter((k) => !propSearch || k.toLowerCase().includes(propSearch.toLowerCase()) || str(props[k]).toLowerCase().includes(propSearch.toLowerCase()))
    .sort((a, b) => Number(a.startsWith("$")) - Number(b.startsWith("$")) || a.localeCompare(b));

  return (
    <div className="page">
      <div className="row small muted" style={{ marginBottom: 10 }}>
        <Link to={path("persons")} className="link">
          Persons
        </Link>
        <Icon name="chevronRight" size={12} />
      </div>
      <div className="page-head" style={{ alignItems: "center" }}>
        {p ? <Avatar name={p.display_name} id={p.id} large /> : <Skeleton width={52} height={52} style={{ borderRadius: "50%" }} />}
        <div className="titles">
          {p ? <h1 className="truncate">{p.display_name}</h1> : <Skeleton height={26} width={260} />}
          <div className="sub row wrap">
            {p && (p.is_identified ? <span className="badge accent">identified</span> : <span className="badge">anonymous</span>)}
            <span className="mono small">{id}</span>
            <CopyButton text={id} label="" className="btn ghost icon small" />
          </div>
        </div>
        <div className="actions">
          <Link className="btn" to={`${path("activity")}?person_id=${encodeURIComponent(id)}`}>
            <Icon name="activity" size={14} /> Live activity
          </Link>
        </div>
      </div>

      <div className="card" style={{ marginBottom: 16 }}>
        <div className="stat-grid">
          {[
            { label: "Events", value: data ? fmtNumber(data.event_count) : null },
            { label: "Sessions", value: data ? fmtNumber(data.session_count) : null },
            { label: "First seen", value: data ? fmtDateTime(data.first_seen ?? p?.created_at) : null },
            { label: "Last seen", value: data ? fmtRelative(data.last_seen) : null },
          ].map((s) => (
            <div key={s.label}>
              <div className="k-label">{s.label}</div>
              <div className="k-value num">{s.value ?? <Skeleton height={20} width={80} />}</div>
            </div>
          ))}
        </div>
      </div>

      <Tabs
        value={tab}
        onChange={setTab}
        options={[
          { value: "events", label: "Events" },
          { value: "properties", label: `Properties${p ? ` (${Object.keys(props).length})` : ""}` },
          { value: "ids", label: `Distinct IDs${data ? ` (${data.distinct_ids.length})` : ""}` },
        ]}
      />
      <div className="card">
        {tab === "events" && <PersonEvents personId={id} />}
        {tab === "properties" && (
          <div className="card-body col gap-12">
            <div className="search" style={{ width: 320 }}>
              <Icon name="search" size={14} />
              <input className="input" placeholder="Search properties…" value={propSearch} onChange={(e) => setPropSearch(e.target.value)} aria-label="Search properties" />
            </div>
            {!data ? (
              <SkeletonRows rows={6} />
            ) : keys.length === 0 ? (
              <Empty icon="info" title={propSearch ? "No matching properties" : "No person properties set"}>
                {!propSearch && <>Set them with <code>posthog.identify(id, {"{ email, plan }"})</code> or <code>$set</code>.</>}
              </Empty>
            ) : (
              <div className="kv">
                {keys.map((k) => (
                  <Fragment key={k}>
                    <div>{k}</div>
                    <div>{str(props[k])}</div>
                  </Fragment>
                ))}
              </div>
            )}
          </div>
        )}
        {tab === "ids" && (
          <div>
            {!data ? (
              <SkeletonRows rows={3} />
            ) : (
              <table className="table compact">
                <tbody>
                  {data.distinct_ids.map((d) => (
                    <tr key={d}>
                      <td className="mono small">{d}</td>
                      <td className="r">
                        <CopyButton text={d} label="" className="btn ghost icon small" />
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
            <p className="muted small" style={{ padding: "10px 16px" }}>
              Every distinct ID merged into this person by <code>$identify</code>, <code>$create_alias</code> or <code>$merge_dangerously</code>.
            </p>
          </div>
        )}
      </div>
      {canEdit(organization) && p && (
        <div className="card card-pad row mt-24" style={{ boxShadow: "0 0 0 1px var(--bad-wash)" }}>
          <div className="grow">
            <h3>Delete person and all their data</h3>
            <p className="secondary small">Erases this person, their distinct IDs and every event they sent. For GDPR deletion requests.</p>
          </div>
          <button className="btn danger" onClick={() => setErasing(true)}>
            <Icon name="trash" size={14} /> Delete person
          </button>
        </div>
      )}
      {erasing && p && <ErasePerson personId={p.id} name={p.display_name} onClose={() => setErasing(false)} />}
    </div>
  );
}
