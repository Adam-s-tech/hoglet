//! Claim 4: person counts are honest.
//!
//! Every property here runs the real identity projection
//! (`projections::apply_captured_event`) on a real SQLite database and checks
//! it against a reference model written independently of the SQL:
//!
//! * `ALL ORDERS` — properties that hold for any event sequence in any order:
//!   every distinct id maps to exactly one person, no person is without
//!   distinct ids, persons <= distinct ids, no distinct id is ever lost,
//!   `$merge_dangerously` always unifies, merges only coarsen the partition,
//!   identified people never merge through `$identify` / `$create_alias`, and
//!   the projection agrees step by step with the flag-aware reference model.
//! * `CONVERGENT` — the class of sequences whose final person graph is the
//!   same in EVERY order, and equal to the connected components of the edges
//!   (a union-find oracle): anonymous -> identified `$identify`s, aliases in
//!   either SDK direction, `$merge_dangerously`, `$set`, plain events, retries.
//!
//! What is NOT claimed to commute (and is pinned honestly by a test below):
//! refusals. `$identify` / `$create_alias` between two *already identified*
//! people refuse to merge them, and whether a person is already identified
//! depends on which event arrived first. Person ids, `created_at` and
//! `first_seen_key` are labels of the first event that touched an id and may
//! also depend on arrival order; the partition of distinct ids is what is
//! claimed.
//!
//! `HOGLET_PROPTEST_CASES=<n>` sets cases per property (default 64).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{Duration, TimeZone, Utc};
use hoglet::capture::event::CapturedEvent;
use hoglet::pipeline::wal::WalCursor;
use hoglet::projections::{apply_captured_event, initialize_schema};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestCaseError, TestRunner};
use rand::SeedableRng;
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rusqlite::Connection;
use serde_json::{Map, Value, json};
use uuid::Uuid;

const PROJECT: &str = "project-1";

fn cases() -> u32 {
    std::env::var("HOGLET_PROPTEST_CASES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(64)
}

fn report(line: &str) {
    println!("{line}");
    use std::io::Write;
    if let Ok(path) = std::env::var("HOGLET_MATRIX_OUT")
        && let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// Run a property, then report how many cases and permutations it covered.
fn property<S: Strategy>(
    name: &str,
    strategy: S,
    test: impl Fn(S::Value, &AtomicU64) -> Result<(), TestCaseError>,
) {
    let permutations = AtomicU64::new(0);
    let mut runner = TestRunner::new(Config {
        cases: cases(),
        failure_persistence: None,
        ..Config::default()
    });
    let run = AtomicU64::new(0);
    let result = runner.run(&strategy, |value| {
        run.fetch_add(1, Ordering::Relaxed);
        test(value, &permutations)
    });
    if let Err(error) = result {
        panic!("{name}: {error}");
    }
    report(&format!(
        "CLAIM4 property={name} cases={} permutations_or_steps={}",
        run.load(Ordering::Relaxed),
        permutations.load(Ordering::Relaxed)
    ));
}

// ---------------------------------------------------------------- events

#[derive(Debug, Clone, PartialEq, Eq)]
enum Op {
    /// A plain event: creates the person on first sight.
    Page(String),
    /// `$identify` as every SDK sends it: `distinct_id = id`, `$anon_distinct_id = anon`.
    Identify { id: String, anon: String },
    /// `$identify` with no anonymous id (a returning user logging in).
    IdentifyBare(String),
    /// `$create_alias`. posthog-node sends `(user, anon)`, posthog-python
    /// sends `(anon, user)`: both orientations are plain `(id, alias)` pairs.
    Alias { id: String, alias: String },
    /// `$merge_dangerously`.
    Merge { id: String, alias: String },
    /// `$set` with a key unique to this event, so no value ever conflicts.
    Set { id: String, key: String },
}

impl Op {
    fn to_event(&self, n: i64) -> CapturedEvent {
        let (name, id, properties) = match self {
            Op::Page(id) => ("$pageview", id, json!({})),
            Op::Identify { id, anon } => ("$identify", id, json!({"$anon_distinct_id": anon})),
            Op::IdentifyBare(id) => ("$identify", id, json!({})),
            Op::Alias { id, alias } => ("$create_alias", id, json!({"alias": alias})),
            Op::Merge { id, alias } => ("$merge_dangerously", id, json!({"alias": alias})),
            Op::Set { id, key } => ("$set", id, json!({"$set": {key: n}})),
        };
        let Value::Object(properties) = properties else { unreachable!() };
        CapturedEvent {
            uuid: Uuid::from_u128(n as u128 + 1),
            event: name.to_owned(),
            distinct_id: id.clone(),
            token: "phc_claim4".to_owned(),
            timestamp: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).single().unwrap() + Duration::seconds(n),
            properties,
        }
    }

    /// Distinct ids the projection must know after this event.
    fn mentions(&self) -> Vec<&str> {
        match self {
            Op::Page(id) | Op::IdentifyBare(id) | Op::Set { id, .. } => vec![id],
            Op::Identify { id, anon: other } | Op::Alias { id, alias: other } | Op::Merge { id, alias: other } => {
                if other.is_empty() || other == id {
                    vec![id]
                } else {
                    vec![id, other]
                }
            }
        }
    }
}

fn database() -> Connection {
    let connection = Connection::open_in_memory().expect("in-memory SQLite");
    connection.execute_batch("PRAGMA foreign_keys=ON;").expect("pragma");
    initialize_schema(&connection).expect("identity schema");
    connection
}

/// Apply `ops` in the given order, one projection transaction per event (as
/// one publication window applies them). Event times follow each event's
/// original position, so a permuted sequence replays the same events, merely
/// in a different order.
fn replay(ops: &[(i64, Op)]) -> Connection {
    let mut connection = database();
    for (n, op) in ops {
        let transaction = connection.transaction().expect("transaction");
        apply_captured_event(&transaction, PROJECT, &op.to_event(*n), WalCursor::origin())
            .expect("projection applies the event");
        transaction.commit().expect("commit");
    }
    connection
}

// -------------------------------------------------- reading the projection

type Partition = BTreeSet<BTreeSet<String>>;

fn partition(connection: &Connection) -> Partition {
    let mut statement = connection
        .prepare("SELECT person_id, distinct_id FROM distinct_ids WHERE project_id = ?1")
        .expect("prepare");
    let mut classes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let rows = statement
        .query_map([PROJECT], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .expect("query");
    for row in rows {
        let (person, distinct_id) = row.expect("row");
        classes.entry(person).or_default().insert(distinct_id);
    }
    classes.into_values().collect()
}

fn count(connection: &Connection, sql: &str) -> u64 {
    connection.query_row(sql, [], |row| row.get::<_, i64>(0)).expect("count") as u64
}

fn identified_of(connection: &Connection, distinct_id: &str) -> Option<bool> {
    connection
        .query_row(
            "SELECT p.is_identified FROM distinct_ids d JOIN persons p
               ON p.project_id = d.project_id AND p.id = d.person_id
             WHERE d.project_id = ?1 AND d.distinct_id = ?2",
            [PROJECT, distinct_id],
            |row| row.get::<_, bool>(0),
        )
        .ok()
}

fn person_of(connection: &Connection, distinct_id: &str) -> Option<String> {
    connection
        .query_row(
            "SELECT person_id FROM distinct_ids WHERE project_id = ?1 AND distinct_id = ?2",
            [PROJECT, distinct_id],
            |row| row.get::<_, String>(0),
        )
        .ok()
}

/// Final `(class -> (identified, properties))`, keyed by the class's ids so
/// it is independent of person-id labels.
fn class_state(connection: &Connection) -> BTreeMap<BTreeSet<String>, (bool, Map<String, Value>)> {
    let mut state = BTreeMap::new();
    for class in partition(connection) {
        let any = class.iter().next().expect("non-empty class");
        let (identified, properties): (bool, String) = connection
            .query_row(
                "SELECT p.is_identified, p.properties FROM distinct_ids d JOIN persons p
                   ON p.project_id = d.project_id AND p.id = d.person_id
                 WHERE d.project_id = ?1 AND d.distinct_id = ?2",
                [PROJECT, any.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("person row");
        let Value::Object(properties) = serde_json::from_str(&properties).expect("json") else {
            panic!("properties are not an object")
        };
        state.insert(class, (identified, properties));
    }
    state
}

// ------------------------------------------------------------- the oracles

#[derive(Clone, Copy, PartialEq, Eq)]
enum Guard {
    RefuseIdentifiedSource,
    RefuseBothIdentified,
    Always,
}

/// Flag-aware reference model of the documented merge rules, in union-find
/// form. Knows nothing about SQL, person ids or property merging.
#[derive(Default, Clone)]
struct Model {
    parent: BTreeMap<String, String>,
    identified: BTreeMap<String, bool>,
}

impl Model {
    fn ensure(&mut self, id: &str) {
        if !self.parent.contains_key(id) {
            self.parent.insert(id.to_owned(), id.to_owned());
            self.identified.insert(id.to_owned(), false);
        }
    }

    fn find(&mut self, id: &str) -> String {
        let mut root = id.to_owned();
        while self.parent[&root] != root {
            root = self.parent[&root].clone();
        }
        let mut cursor = id.to_owned();
        while self.parent[&cursor] != root {
            let next = self.parent[&cursor].clone();
            self.parent.insert(cursor, root.clone());
            cursor = next;
        }
        root
    }

    fn merge(&mut self, loser_id: &str, winner_id: &str, guard: Guard) {
        self.ensure(winner_id);
        self.ensure(loser_id);
        let (winner, loser) = (self.find(winner_id), self.find(loser_id));
        if winner == loser {
            return;
        }
        let (winner_identified, loser_identified) = (self.identified[&winner], self.identified[&loser]);
        match guard {
            Guard::RefuseIdentifiedSource if loser_identified => return,
            Guard::RefuseBothIdentified if loser_identified && winner_identified => return,
            _ => {}
        }
        self.parent.insert(loser.clone(), winner.clone());
        self.identified.insert(winner, true);
    }

    fn mark_identified(&mut self, id: &str) {
        let root = self.find(id);
        self.identified.insert(root, true);
    }

    fn apply(&mut self, op: &Op) {
        match op {
            Op::Page(id) | Op::Set { id, .. } => self.ensure(id),
            Op::IdentifyBare(id) => {
                self.ensure(id);
                self.mark_identified(id);
            }
            Op::Identify { id, anon } => {
                self.ensure(id);
                if !anon.is_empty() && anon != id {
                    self.merge(anon, id, Guard::RefuseIdentifiedSource);
                }
                self.mark_identified(id);
            }
            Op::Alias { id, alias } => {
                self.ensure(id);
                if !alias.is_empty() && alias != id {
                    self.merge(alias, id, Guard::RefuseBothIdentified);
                }
                self.mark_identified(id);
            }
            Op::Merge { id, alias } => {
                self.ensure(id);
                if !alias.is_empty() && alias != id {
                    self.merge(alias, id, Guard::Always);
                }
            }
        }
    }

    fn partition(&mut self) -> Partition {
        let ids: Vec<String> = self.parent.keys().cloned().collect();
        let mut classes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for id in ids {
            let root = self.find(&id);
            classes.entry(root).or_default().insert(id);
        }
        classes.into_values().collect()
    }
}

/// Plain connected components of undirected edges, with no notion of
/// identification: the oracle for the CONVERGENT class, where no merge may
/// be refused.
fn components(ids: &BTreeSet<String>, edges: &[(String, String)]) -> Partition {
    let mut group: BTreeMap<&String, usize> = ids.iter().enumerate().map(|(i, id)| (id, i)).collect();
    let mut changed = true;
    while changed {
        changed = false;
        for (a, b) in edges {
            let (ga, gb) = (group[a], group[b]);
            if ga != gb {
                let (keep, replace) = (ga.min(gb), ga.max(gb));
                for value in group.values_mut() {
                    if *value == replace {
                        *value = keep;
                    }
                }
                changed = true;
            }
        }
    }
    let mut classes: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
    for (id, g) in group {
        classes.entry(g).or_default().insert(id.clone());
    }
    classes.into_values().collect()
}

// ----------------------------------------------- the invariants (ALL ORDERS)

/// Structural truths that must hold after every event, in any order.
fn structural_invariants(connection: &Connection, expected_ids: &BTreeSet<String>) -> Result<(), TestCaseError> {
    let ids: BTreeSet<String> = {
        let mut statement = connection.prepare("SELECT distinct_id FROM distinct_ids").unwrap();
        statement.query_map([], |row| row.get(0)).unwrap().map(|r| r.unwrap()).collect()
    };
    prop_assert_eq!(&ids, expected_ids, "a distinct id was lost or invented");
    let rows = count(connection, "SELECT count(*) FROM distinct_ids");
    prop_assert_eq!(rows as usize, ids.len(), "a distinct id maps to more than one person");
    let dangling = count(
        connection,
        "SELECT count(*) FROM distinct_ids d LEFT JOIN persons p
           ON p.project_id = d.project_id AND p.id = d.person_id WHERE p.id IS NULL",
    );
    prop_assert_eq!(dangling, 0, "a distinct id points at a person that does not exist");
    let ghosts = count(
        connection,
        "SELECT count(*) FROM persons p WHERE NOT EXISTS
           (SELECT 1 FROM distinct_ids d WHERE d.project_id = p.project_id AND d.person_id = p.id)",
    );
    prop_assert_eq!(ghosts, 0, "a ghost person has no distinct id");
    let persons = count(connection, "SELECT count(*) FROM persons");
    let classes = partition(connection).len() as u64;
    prop_assert_eq!(persons, classes, "person count differs from the number of distinct-id classes");
    prop_assert!(persons as usize <= ids.len(), "more persons than distinct ids");
    Ok(())
}

fn finer_or_equal(before: &Partition, after: &Partition) -> bool {
    before
        .iter()
        .all(|class| after.iter().any(|bigger| class.is_subset(bigger)))
}

/// Reconciliation check (claim 4b): the number of persons equals the number
/// of classes an independent reference model computes from the events, the
/// reader-facing mapping agrees, and the numbers are exposed for inspection.
fn reconcile_persons(connection: &Connection, ops: &[(i64, Op)]) -> Result<(u64, u64), String> {
    let mut model = Model::default();
    for (_, op) in ops {
        model.apply(op);
    }
    let expected = model.partition();
    let actual = partition(connection);
    if actual != expected {
        return Err(format!("persons differ from the reference model: {actual:?} vs {expected:?}"));
    }
    let persons = count(connection, "SELECT count(*) FROM persons");
    let ids = count(connection, "SELECT count(*) FROM distinct_ids");
    if persons != expected.len() as u64 {
        return Err(format!("{persons} persons for {} classes", expected.len()));
    }
    Ok((persons, ids))
}

// --------------------------------------------------------------- generators

fn id(universe: usize) -> impl Strategy<Value = String> {
    (0..universe).prop_map(|n| format!("id{n}"))
}

fn any_op(universe: usize) -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => id(universe).prop_map(Op::Page),
        4 => (id(universe), id(universe)).prop_map(|(id, anon)| Op::Identify { id, anon }),
        1 => id(universe).prop_map(Op::IdentifyBare),
        3 => (id(universe), id(universe)).prop_map(|(id, alias)| Op::Alias { id, alias }),
        2 => (id(universe), id(universe)).prop_map(|(id, alias)| Op::Merge { id, alias }),
        2 => id(universe).prop_map(|id| Op::Set { id, key: String::new() }),
    ]
}

fn numbered(ops: Vec<Op>) -> Vec<(i64, Op)> {
    ops.into_iter()
        .enumerate()
        .map(|(n, op)| {
            let op = match op {
                Op::Set { id, .. } => Op::Set { id, key: format!("k{n}") },
                other => other,
            };
            (n as i64, op)
        })
        .collect()
}

// ------------------------------------------------- property: ALL ORDERS

#[test]
fn all_orders_projection_matches_the_model_and_keeps_every_structural_invariant() {
    property(
        "all_orders_step_invariants",
        (3_usize..9, proptest::collection::vec(any_op(8), 1..40), any::<u64>(), 1_usize..4),
        |(universe, ops, seed, shuffles), steps| {
            let base = numbered(ops.into_iter().map(|op| clamp(op, universe)).collect());
            // The original order and a few random permutations: every one
            // must satisfy every invariant (they just need not agree).
            let mut orders = vec![base.clone()];
            let mut rng = StdRng::seed_from_u64(seed);
            for _ in 0..shuffles {
                let mut shuffled = base.clone();
                shuffled.shuffle(&mut rng);
                orders.push(shuffled);
            }
            for order in orders {
                let mut connection = database();
                let mut model = Model::default();
                let mut known: BTreeSet<String> = BTreeSet::new();
                let mut previous = Partition::new();
                for (n, op) in &order {
                    steps.fetch_add(1, Ordering::Relaxed);
                    // Pre-state, read from the database (not from the model).
                    let pre = |id: &str| (person_of(&connection, id), identified_of(&connection, id));
                    let pre_state = match op {
                        Op::Identify { id, anon } | Op::Alias { id, alias: anon } | Op::Merge { id, alias: anon } => {
                            Some((pre(id), pre(anon)))
                        }
                        _ => None,
                    };
                    let transaction = connection.transaction().unwrap();
                    apply_captured_event(&transaction, PROJECT, &op.to_event(*n), WalCursor::origin()).unwrap();
                    transaction.commit().unwrap();
                    model.apply(op);
                    known.extend(op.mentions().into_iter().map(str::to_owned));

                    structural_invariants(&connection, &known)?;
                    let now = partition(&connection);
                    prop_assert!(finer_or_equal(&previous, &now), "a merge split an existing person");
                    prop_assert_eq!(&now, &model.partition(), "projection and reference model diverged on {:?}", op);
                    previous = now;

                    // Rule-by-rule, from the pre-state flags.
                    if let Some(((id_person, id_identified), (other_person, other_identified))) = pre_state {
                        let (id, other) = match op {
                            Op::Identify { id, anon } => (id, anon),
                            Op::Alias { id, alias } | Op::Merge { id, alias } => (id, alias),
                            _ => unreachable!(),
                        };
                        if !other.is_empty() && other != id {
                            let joined = person_of(&connection, id) == person_of(&connection, other);
                            let separate_before = id_person != other_person || id_person.is_none();
                            match op {
                                Op::Merge { .. } => prop_assert!(joined, "$merge_dangerously did not unify {id} and {other}"),
                                Op::Identify { .. } => {
                                    if other_identified == Some(true) && separate_before {
                                        prop_assert!(!joined, "$identify merged an identified person ({other})");
                                    } else {
                                        prop_assert!(joined, "$identify of {id} failed to absorb anonymous {other}");
                                    }
                                }
                                Op::Alias { .. } => {
                                    if id_identified == Some(true) && other_identified == Some(true) && separate_before {
                                        prop_assert!(!joined, "$create_alias merged two identified people");
                                    } else {
                                        prop_assert!(joined, "$create_alias failed to join {id} and {other}");
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                // Final check: persons == classes of an independent oracle.
                if let Err(error) = reconcile_persons(&connection, &order) {
                    return Err(TestCaseError::fail(error));
                }
            }
            Ok(())
        },
    );
}

/// Keep every id inside `universe` (the strategy draws from the maximum).
fn clamp(op: Op, universe: usize) -> Op {
    let fix = |id: String| {
        let n: usize = id[2..].parse().unwrap();
        format!("id{}", n % universe)
    };
    match op {
        Op::Page(id) => Op::Page(fix(id)),
        Op::IdentifyBare(id) => Op::IdentifyBare(fix(id)),
        Op::Identify { id, anon } => Op::Identify { id: fix(id), anon: fix(anon) },
        Op::Alias { id, alias } => Op::Alias { id: fix(id), alias: fix(alias) },
        Op::Merge { id, alias } => Op::Merge { id: fix(id), alias: fix(alias) },
        Op::Set { id, key } => Op::Set { id: fix(id), key },
    }
}

// ----------------------------------------------- property: CONVERGENT

/// How a leaf (an anonymous or alias id touched by exactly one identity
/// event) is attached to a hub (a long-lived id such as a user).
#[derive(Debug, Clone, Copy)]
enum Attach {
    /// `$identify` of the hub, `$anon_distinct_id` = leaf.
    Identify,
    /// posthog-node: `$create_alias(distinct_id = hub, alias = leaf)`.
    AliasNode,
    /// posthog-python: `$create_alias(distinct_id = leaf, alias = hub)`.
    AliasPython,
    /// `$merge_dangerously(distinct_id = hub, alias = leaf)`.
    MergeHubFirst,
    /// `$merge_dangerously(distinct_id = leaf, alias = hub)`.
    MergeLeafFirst,
}

#[derive(Debug, Clone)]
struct Convergent {
    hubs: usize,
    leaves: Vec<(usize, Attach)>,
    hub_merges: Vec<(usize, usize)>,
    bare_identifies: Vec<usize>,
    pages: Vec<usize>,
    sets: Vec<usize>,
    retries: Vec<usize>,
}

fn convergent() -> impl Strategy<Value = Convergent> {
    let attach = prop_oneof![
        Just(Attach::Identify),
        Just(Attach::AliasNode),
        Just(Attach::AliasPython),
        Just(Attach::MergeHubFirst),
        Just(Attach::MergeLeafFirst),
    ];
    (
        2_usize..6,
        proptest::collection::vec((0_usize..6, attach), 0..24),
        proptest::collection::vec((0_usize..6, 0_usize..6), 0..4),
        proptest::collection::vec(0_usize..6, 0..3),
        proptest::collection::vec(0_usize..40, 0..12),
        proptest::collection::vec(0_usize..40, 0..8),
        proptest::collection::vec(0_usize..60, 0..10),
    )
        .prop_map(|(hubs, leaves, hub_merges, bare, pages, sets, retries)| Convergent {
            hubs,
            leaves: leaves.into_iter().map(|(hub, kind)| (hub % hubs, kind)).collect(),
            hub_merges: hub_merges.into_iter().map(|(a, b)| (a % hubs, b % hubs)).collect(),
            bare_identifies: bare.into_iter().map(|h| h % hubs).collect(),
            pages,
            sets,
            retries,
        })
}

impl Convergent {
    /// The events, and the edges a union-find oracle needs.
    fn build(&self) -> (Vec<Op>, BTreeSet<String>, Vec<(String, String)>) {
        let hub = |h: usize| format!("hub{h}");
        let leaf = |l: usize| format!("leaf{l}");
        let mut ops = Vec::new();
        let mut edges = Vec::new();
        let mut ids: BTreeSet<String> = (0..self.hubs).map(hub).collect();
        for (l, (h, kind)) in self.leaves.iter().enumerate() {
            let (hub_id, leaf_id) = (hub(*h), leaf(l));
            ids.insert(leaf_id.clone());
            edges.push((hub_id.clone(), leaf_id.clone()));
            ops.push(match kind {
                Attach::Identify => Op::Identify { id: hub_id, anon: leaf_id },
                Attach::AliasNode => Op::Alias { id: hub_id, alias: leaf_id },
                Attach::AliasPython => Op::Alias { id: leaf_id, alias: hub_id },
                Attach::MergeHubFirst => Op::Merge { id: hub_id, alias: leaf_id },
                Attach::MergeLeafFirst => Op::Merge { id: leaf_id, alias: hub_id },
            });
        }
        for (a, b) in &self.hub_merges {
            if a != b {
                ops.push(Op::Merge { id: hub(*a), alias: hub(*b) });
                edges.push((hub(*a), hub(*b)));
            }
        }
        for h in &self.bare_identifies {
            ops.push(Op::IdentifyBare(hub(*h)));
        }
        let all: Vec<String> = ids.iter().cloned().collect();
        for p in &self.pages {
            ops.push(Op::Page(all[p % all.len()].clone()));
        }
        for s in &self.sets {
            ops.push(Op::Set { id: all[s % all.len()].clone(), key: String::new() });
        }
        // SDK retries: the same identity event delivered again.
        let identity: Vec<Op> = ops
            .iter()
            .filter(|op| !matches!(op, Op::Page(_) | Op::Set { .. }))
            .cloned()
            .collect();
        for r in &self.retries {
            if !identity.is_empty() {
                ops.push(identity[r % identity.len()].clone());
            }
        }
        (ops, ids, edges)
    }
}

#[test]
fn convergent_class_gives_the_same_person_graph_in_every_order() {
    property(
        "convergent_every_order",
        (convergent(), any::<u64>(), 3_usize..9),
        |(shape, seed, shuffles), permutations| {
            let (ops, ids, edges) = shape.build();
            prop_assume!(!ops.is_empty());
            let base = numbered(ops);
            let expected = components(&ids_seen(&base, &ids), &edges);
            // What the oracle says about identification and properties.
            let bare: BTreeSet<String> = base
                .iter()
                .filter_map(|(_, op)| if let Op::IdentifyBare(id) = op { Some(id.clone()) } else { None })
                .collect();
            let mut expected_keys: BTreeMap<BTreeSet<String>, BTreeSet<String>> = BTreeMap::new();
            for class in &expected {
                expected_keys.insert(class.clone(), BTreeSet::new());
            }
            for (_, op) in &base {
                if let Op::Set { id, key } = op {
                    let class = expected.iter().find(|c| c.contains(id)).expect("class");
                    expected_keys.get_mut(class).unwrap().insert(key.clone());
                }
            }

            let mut rng = StdRng::seed_from_u64(seed);
            let mut orders = vec![base.clone(), base.iter().rev().cloned().collect()];
            for _ in 0..shuffles {
                let mut shuffled = base.clone();
                shuffled.shuffle(&mut rng);
                orders.push(shuffled);
            }
            let mut first_state = None;
            for order in orders {
                permutations.fetch_add(1, Ordering::Relaxed);
                let connection = replay(&order);
                structural_invariants(&connection, &ids_seen(&base, &ids))?;
                let actual = partition(&connection);
                prop_assert_eq!(&actual, &expected, "final person graph differs from the union-find oracle");
                // Persons == classes, via the independent model too.
                if let Err(error) = reconcile_persons(&connection, &order) {
                    return Err(TestCaseError::fail(error));
                }
                let state = class_state(&connection);
                for (class, (identified, properties)) in &state {
                    let keys: BTreeSet<String> = properties.keys().cloned().collect();
                    prop_assert_eq!(&keys, &expected_keys[class], "$set keys were lost or invented in {:?}", class);
                    let should = class.len() > 1 || class.iter().any(|id| bare.contains(id));
                    prop_assert_eq!(*identified, should, "is_identified of {:?} depends on order", class);
                }
                match &first_state {
                    None => first_state = Some(state),
                    Some(first) => prop_assert_eq!(&state, first, "full person state differs between orders"),
                }
            }
            Ok(())
        },
    );
}

fn ids_seen(ops: &[(i64, Op)], declared: &BTreeSet<String>) -> BTreeSet<String> {
    // Only ids an event actually mentioned exist in the projection.
    let mut seen = BTreeSet::new();
    for (_, op) in ops {
        seen.extend(op.mentions().into_iter().map(str::to_owned));
    }
    seen.retain(|id| declared.contains(id));
    seen
}

// ---------------------------------------- reconciliation on a big dataset

#[test]
fn reconciliation_on_large_random_datasets_counts_every_person_once() {
    use rand::Rng;
    let datasets = (cases() / 8).clamp(4, 400) as u64;
    let mut checked = 0;
    for seed in 0..datasets {
        let mut rng = StdRng::seed_from_u64(seed);
        let universe = rng.gen_range(50..400);
        let events = rng.gen_range(300..1500);
        let pick = |rng: &mut StdRng| format!("u{}", rng.gen_range(0..universe));
        let mut ops = Vec::new();
        for _ in 0..events {
            let op = match rng.gen_range(0..10) {
                0..=3 => Op::Page(pick(&mut rng)),
                4..=6 => Op::Identify { id: pick(&mut rng), anon: pick(&mut rng) },
                7 => Op::Alias { id: pick(&mut rng), alias: pick(&mut rng) },
                8 => Op::Merge { id: pick(&mut rng), alias: pick(&mut rng) },
                _ => Op::IdentifyBare(pick(&mut rng)),
            };
            ops.push(op);
        }
        let ops = numbered(ops);
        let connection = replay(&ops);
        let (persons, ids) = reconcile_persons(&connection, &ops).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
        let every: BTreeSet<String> = ops.iter().flat_map(|(_, op)| op.mentions().into_iter().map(str::to_owned)).collect();
        structural_invariants(&connection, &every).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
        assert!(persons <= ids, "seed {seed}: ratio inverted");
        checked += 1;
        if seed == 0 {
            report(&format!(
                "CLAIM4 reconciliation sample: seed=0 events={events} distinct_ids={ids} persons={persons} ratio={:.3}",
                ids as f64 / persons as f64
            ));
        }
    }
    report(&format!("CLAIM4 property=reconciliation_large datasets={checked}"));
}

// ------------------------------------------- what is NOT claimed, pinned

#[test]
fn refusals_between_identified_people_depend_on_arrival_order_and_are_not_claimed_to_commute() {
    // a -> b then b -> c: b is identified by the first identify, so the
    // second one finds an identified anonymous side and refuses.
    let first = Op::Identify { id: "b".into(), anon: "a".into() };
    let second = Op::Identify { id: "c".into(), anon: "b".into() };
    let one_way = partition(&replay(&numbered(vec![first.clone(), second.clone()])));
    let other_way = partition(&replay(&numbered(vec![second, first])));
    assert_ne!(one_way, other_way, "if these ever converge, claims.md can claim more");
    // Honest guarantee that still holds in both orders: no distinct id lost,
    // no ghost, and the two identified users are not merged by identify.
    for graph in [&one_way, &other_way] {
        let all: BTreeSet<&String> = graph.iter().flatten().collect();
        assert_eq!(all.len(), 3);
    }
}

// ----------------------------------------------- adversarial scenarios

fn at(ops: Vec<Op>) -> Vec<(i64, Op)> {
    numbered(ops)
}

fn identify(id: &str, anon: &str) -> Op {
    Op::Identify { id: id.into(), anon: anon.into() }
}

fn page(id: &str) -> Op {
    Op::Page(id.into())
}

fn classes(connection: &Connection) -> Vec<Vec<String>> {
    partition(connection).into_iter().map(|c| c.into_iter().collect()).collect()
}

#[test]
fn shared_device_two_users_logging_in_never_merge_in_either_order() {
    // One browser (anonymous id `device`) used by alice then bob, or bob then alice.
    for order in [["alice", "bob"], ["bob", "alice"]] {
        let ops = at(vec![
            page("device"),
            identify(order[0], "device"),
            page("device"),
            identify(order[1], "device"),
        ]);
        let connection = replay(&ops);
        let persons = classes(&connection);
        assert_eq!(persons.len(), 2, "alice and bob must stay two people: {persons:?}");
        let alice = person_of(&connection, "alice").unwrap();
        let bob = person_of(&connection, "bob").unwrap();
        assert_ne!(alice, bob);
        // `device` belongs to exactly one of them (a function, not a relation).
        let owner = person_of(&connection, "device").unwrap();
        assert!(owner == alice || owner == bob);
        assert_eq!(count(&connection, "SELECT count(*) FROM distinct_ids WHERE distinct_id = 'device'"), 1);
        assert_eq!(count(&connection, "SELECT count(*) FROM persons"), 2);
    }
}

#[test]
fn cleared_cookies_new_anonymous_id_joins_the_same_person_exactly_once() {
    let ops = at(vec![
        page("anon1"),
        identify("alice", "anon1"),
        // cookies cleared: a new anonymous id, the user logs in again
        page("anon2"),
        identify("alice", "anon2"),
        // and again, and the SDK retries the same identify
        identify("alice", "anon2"),
        page("anon3"),
        identify("alice", "anon3"),
    ]);
    let connection = replay(&ops);
    assert_eq!(classes(&connection), vec![vec!["alice", "anon1", "anon2", "anon3"]]);
    assert_eq!(count(&connection, "SELECT count(*) FROM persons"), 1);
}

#[test]
fn identify_before_and_after_pageviews_gives_one_person() {
    // Pageviews keep carrying the old anonymous id after identify, and
    // events for the identified id may arrive before the identify.
    let before = replay(&at(vec![page("a"), page("a"), identify("u", "a"), page("a"), page("u")]));
    let after = replay(&at(vec![identify("u", "a"), page("a"), page("u"), page("a")]));
    assert_eq!(partition(&before), partition(&after));
    assert_eq!(classes(&before), vec![vec!["a", "u"]]);
    assert_eq!(count(&before, "SELECT count(*) FROM persons"), 1);
}

#[test]
fn both_sdk_alias_directions_keep_the_identified_person_and_its_properties() {
    // posthog-node: alias(distinctId = user, alias = anonymous).
    // posthog-python: alias(previous_id = anonymous, distinct_id = user).
    let user_first = |alias: Op| {
        at(vec![
            Op::IdentifyBare("user".into()),
            Op::Set { id: "user".into(), key: String::new() },
            page("anon"),
            alias,
        ])
    };
    let node = replay(&user_first(Op::Alias { id: "user".into(), alias: "anon".into() }));
    let python = replay(&user_first(Op::Alias { id: "anon".into(), alias: "user".into() }));
    for connection in [&node, &python] {
        assert_eq!(classes(connection), vec![vec!["anon", "user"]]);
        // The identified person survives: same person id, properties kept.
        assert_eq!(person_of(connection, "anon").as_deref(), Some("user"));
        assert_eq!(identified_of(connection, "anon"), Some(true));
        let properties: String = connection
            .query_row("SELECT properties FROM persons WHERE id = 'user'", [], |row| row.get(0))
            .unwrap();
        assert!(properties.contains("k1"), "the user's properties were lost: {properties}");
    }
    assert_eq!(partition(&node), partition(&python));

    // Aliasing two identified people is refused in BOTH directions...
    for (id, alias) in [("u1", "u2"), ("u2", "u1")] {
        let connection = replay(&at(vec![
            Op::IdentifyBare("u1".into()),
            Op::IdentifyBare("u2".into()),
            Op::Alias { id: id.into(), alias: alias.into() },
        ]));
        assert_eq!(classes(&connection).len(), 2, "alias({id},{alias}) merged two identified people");
    }
    // ...while $merge_dangerously unifies them in both.
    for (id, alias) in [("u1", "u2"), ("u2", "u1")] {
        let connection = replay(&at(vec![
            Op::IdentifyBare("u1".into()),
            Op::IdentifyBare("u2".into()),
            Op::Merge { id: id.into(), alias: alias.into() },
        ]));
        assert_eq!(classes(&connection).len(), 1, "merge_dangerously({id},{alias}) failed to unify");
    }
}

/// Concurrent identifies of the same people through the real durable
/// pipeline: arrival order in the WAL is arbitrary, the person graph is not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_identifies_of_one_person_through_the_real_pipeline_converge() {
    use std::sync::Arc;

    use hoglet::lake::Lake;
    use hoglet::persons::PersonStore;
    use hoglet::sink::{AuthorizedEventBatch, DurableWalSink, EventSink, PipelineConfig};
    use hoglet::storage_bootstrap::{StoragePaths, bootstrap_storage};

    const TOKEN: &str = "phc_claim4";
    for round in 0..5_u64 {
        let directory = tempfile::tempdir().unwrap();
        let paths = StoragePaths::new(directory.path());
        bootstrap_storage(paths.control(), paths.projections()).unwrap();
        let lake = Arc::new(Lake::open(&paths.projections(), &directory.path().join("events")).unwrap());
        let persons = PersonStore::open(&paths.projections()).unwrap();
        let (sink, runtime, _) = DurableWalSink::open(
            PipelineConfig {
                wal_dir: directory.path().join("wal"),
                tmp_dir: directory.path().join("tmp"),
                retention_days: None,
            },
            lake,
        )
        .unwrap();
        let project_id = Uuid::new_v4().to_string();

        // 3 users, each logging in from 20 anonymous browsers at once.
        let mut ops = Vec::new();
        let mut expected_classes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for user in 0..3 {
            let user_id = format!("user{user}");
            expected_classes.entry(user_id.clone()).or_default().insert(user_id.clone());
            for browser in 0..20 {
                let anon = format!("anon-{user}-{browser}");
                expected_classes.get_mut(&user_id).unwrap().insert(anon.clone());
                ops.push(page(&anon));
                ops.push(identify(&user_id, &anon));
                ops.push(page(&user_id));
            }
        }
        let mut rng = StdRng::seed_from_u64(round);
        ops.shuffle(&mut rng);

        let sink = Arc::new(sink);
        let mut tasks = Vec::new();
        for (slot, chunk) in ops.chunks(7).enumerate() {
            let sink = sink.clone();
            let events: Vec<CapturedEvent> = chunk
                .iter()
                .enumerate()
                .map(|(i, op)| op.to_event((slot * 7 + i) as i64))
                .collect();
            let mut bindings = BTreeMap::new();
            bindings.insert(TOKEN.to_owned(), project_id.clone());
            tasks.push(tokio::spawn(async move {
                sink.append(AuthorizedEventBatch {
                    events,
                    project_ids_by_token: bindings,
                    historical_migration: false,
                })
                .await
            }));
        }
        for task in tasks {
            task.await.unwrap().expect("acknowledged");
        }
        runtime.shutdown().await.unwrap();

        // The reader-facing view (what queries and flags use).
        let all_ids: Vec<String> = expected_classes.values().flatten().cloned().collect();
        let resolved = persons.resolve_distinct_ids(&project_id, &all_ids).unwrap();
        assert_eq!(resolved.len(), all_ids.len(), "round {round}: an id has no person");
        let mut seen: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (distinct_id, person) in &resolved {
            seen.entry(person.clone()).or_default().insert(distinct_id.clone());
        }
        let got: BTreeSet<BTreeSet<String>> = seen.into_values().collect();
        let want: BTreeSet<BTreeSet<String>> = expected_classes.into_values().collect();
        assert_eq!(got, want, "round {round}: the concurrent identifies produced a different person graph");
        let direct = Connection::open(paths.projections()).unwrap();
        assert_eq!(count(&direct, "SELECT count(*) FROM persons"), 3, "round {round}: ghost persons");
    }
}
