// Stored-outcome steps shared by the server-SDK suites (node, python). Both
// drive the same identity script — capture as `anon`, $identify linking
// `anon` → `user`, capture as `anon2`, alias `anon2` → `user`, $set and
// $set_once — and must land the same graph.

import { assertDauIsOne, listEvents, poll, trends, waitForEvents, waitForSinglePerson } from "./harness.mjs";
import { eq, fail, ok, subset } from "./report.mjs";

export const VIEWED_PROPS = (run) => ({
  ct_run: run,
  plan_viewed: "pro",
  n: 42,
  ratio: 0.25,
  ok: true,
  nested: { a: [1, 2], b: { c: "d" } },
  empty: null,
});

/// `ids`: { anon, anon2, user, company, run }; `uuids`: { viewed, grouped, anon2, after }.
export async function storedOutcomeSteps(report, api, projectId, { ids, uuids, sentTs }) {
  const stored = await report.step("stored: every captured event is listed by GET /events", () =>
    waitForEvents(api, projectId, Object.values(uuids))
  );
  const byUuid = (u) => stored?.find((e) => e.uuid === u) ?? fail("event not stored", null, u);

  await report.step("stored: event name, distinct_id and properties are intact", () => {
    const e = byUuid(uuids.viewed);
    eq(e.event, "ct_signup_viewed", "event");
    eq(e.distinct_id, ids.anon, "distinct_id");
    subset(e.properties, VIEWED_PROPS(ids.run), "properties");
  });
  await report.step("stored: client timestamp preserved (within 1s; event sent 60s in the past)", () => {
    const e = byUuid(uuids.viewed);
    const delta = Math.abs(Date.parse(e.timestamp) - sentTs.getTime());
    ok(delta <= 1000, "timestamp drift", { stored: e.timestamp, sent: sentTs.toISOString() }, "|stored - sent| <= 1000ms");
  });
  await report.step("stored: groups arrive as $groups", () => {
    eq(byUuid(uuids.grouped).properties?.$groups, { company: ids.company }, "$groups");
  });
  await report.step("stored: $identify and $create_alias events are listed", async () => {
    const names = new Set((await listEvents(api, projectId)).map((e) => e.event));
    eq({ identify: names.has("$identify"), alias: names.has("$create_alias") }, { identify: true, alias: true }, "special events present");
  });

  const person = await report.step("stored: $identify + alias merged anon, anon2 and user into ONE person", () =>
    waitForSinglePerson(api, projectId, [ids.anon, ids.anon2, ids.user])
  );
  await report.step("stored: $set applied and $set_once not overwritten", () => {
    if (!person) fail("no merged person", null, "person");
    subset(person.person.properties, { plan: "enterprise", email: ids.user, seats: 5, first_plan: "free" }, "person properties");
  });
  await report.step("stored: the person's events include the anonymous history", async () => {
    if (!person) fail("no merged person", null, "person");
    const { json } = await api.get(`/api/projects/${projectId}/persons/${person.person.id}/events?limit=200`);
    const got = new Set((json?.events ?? []).map((e) => e.uuid));
    eq(
      Object.fromEntries(Object.entries(uuids).map(([k, u]) => [k, got.has(u)])),
      Object.fromEntries(Object.keys(uuids).map((k) => [k, true])),
      "events on person"
    );
  });
  await report.step("stored: TrendsQuery math=dau over all events counts 1 person", () => assertDauIsOne(api, projectId));
  await report.step("stored: TrendsQuery math=total over ct_run events counts every event", () =>
    poll(async () => {
      const series = await trends(api, projectId, [
        { event: null, math: "total", properties: [{ key: "ct_run", type: "event", operator: "exact", value: ids.run }] },
      ]);
      eq(series[0]?.aggregated_value, Object.keys(uuids).length, "aggregated_value");
    })
  );
}
