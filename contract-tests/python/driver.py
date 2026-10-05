"""Drives posthog-python `latest`, unmodified, for python.test.mjs.

One scenario per process. Configuration comes from the environment; the
SDK-observable results are printed as one JSON line on stdout. All
assertions live in python.test.mjs so every suite shares one reporter and
one set of stored-outcome checks.
"""

import json
import os
import sys
import time
from datetime import datetime, timedelta, timezone

import posthog
from posthog import Posthog

HOST = os.environ["CT_HOST"]
TOKEN = os.environ["CT_TOKEN"]
RUN = os.environ["CT_RUN"]
ANON = f"anon-{RUN}"
ANON2 = f"anon2-{RUN}"
USER = f"user-{RUN}@contract.test"
NO_EVENTS = {"send_feature_flag_events": False}


def guard(fn):
    """SDK exceptions are results too: report them instead of dying."""
    try:
        return fn()
    except Exception as error:  # noqa: BLE001
        return {"__error__": f"{type(error).__name__}: {error}"}


def flag_ids():
    return [f"mv-{RUN}-{i}" for i in range(12)]


def evaluate(client, pp):
    out = {
        "on": guard(lambda: client.feature_enabled("ct-bool-on", USER, person_properties=pp, **NO_EVENTS)),
        "off": guard(lambda: client.feature_enabled("ct-bool-off", USER, person_properties=pp, **NO_EVENTS)),
        "inactive": guard(lambda: client.feature_enabled("ct-inactive", USER, person_properties=pp, **NO_EVENTS)),
        "plan_on": guard(
            lambda: client.feature_enabled("ct-person-plan", f"pp-{RUN}", person_properties={"plan": "enterprise"}, **NO_EVENTS)
        ),
        "plan_off": guard(
            lambda: client.feature_enabled("ct-person-plan", f"pp-{RUN}", person_properties={"plan": "free"}, **NO_EVENTS)
        ),
        "variants": {i: guard(lambda i=i: client.get_feature_flag("ct-multivariate", i, person_properties=pp, **NO_EVENTS)) for i in flag_ids()},
        "payload_on": guard(lambda: client.get_feature_flag_payload("ct-bool-on", USER, person_properties=pp)),
        "payload_mv": guard(lambda: client.get_feature_flag_payload("ct-multivariate", USER, person_properties=pp)),
        "all": guard(lambda: client.get_all_flags(USER, person_properties=pp)),
        "all_and_payloads": guard(lambda: client.get_all_flags_and_payloads(USER, person_properties=pp)),
    }
    return out


def main_scenario():
    client = Posthog(TOKEN, host=HOST)
    sent_ts = datetime.now(timezone.utc) - timedelta(seconds=60)
    uuids = {k: os.environ[f"CT_UUID_{k.upper()}"] for k in ("viewed", "grouped", "anon2", "after")}
    viewed = {"ct_run": RUN, "plan_viewed": "pro", "n": 42, "ratio": 0.25, "ok": True, "nested": {"a": [1, 2], "b": {"c": "d"}}, "empty": None}
    client.capture("ct_signup_viewed", distinct_id=ANON, properties=viewed, timestamp=sent_ts, uuid=uuids["viewed"])
    client.capture("ct_grouped", distinct_id=ANON, properties={"ct_run": RUN}, groups={"company": f"acme-{RUN}"}, uuid=uuids["grouped"])
    # posthog-python has no identify(); PostHog's documented server-side
    # equivalent is a $identify event carrying $anon_distinct_id.
    client.capture(
        "$identify",
        distinct_id=USER,
        properties={"$anon_distinct_id": ANON, "$set": {"plan": "enterprise", "email": USER}, "$set_once": {"first_plan": "free"}},
    )
    client.capture("ct_anon2_event", distinct_id=ANON2, properties={"ct_run": RUN}, uuid=uuids["anon2"])
    client.alias(previous_id=ANON2, distinct_id=USER)
    client.set(distinct_id=USER, properties={"seats": 5})
    client.set_once(distinct_id=USER, properties={"first_plan": "pro"})
    client.capture("ct_after_login", distinct_id=USER, properties={"ct_run": RUN}, uuid=uuids["after"])
    flush_error = guard(lambda: client.flush())

    results = evaluate(client, {"plan": "enterprise"})
    results["sent_ts"] = sent_ts.isoformat()
    results["flush"] = flush_error
    # Stored person property path: no person_properties passed at all.
    deadline = time.time() + 10
    stored = None
    while time.time() < deadline:
        stored = guard(lambda: client.feature_enabled("ct-person-plan", USER, **NO_EVENTS))
        if stored is True:
            break
        time.sleep(0.3)
    results["plan_from_stored_person"] = stored
    client.shutdown()
    return results


def local_scenario():
    client = Posthog(TOKEN, host=HOST, personal_api_key=os.environ["CT_PERSONAL_KEY"], poll_interval=60)
    load = guard(lambda: client.load_feature_flags())
    definitions = guard(lambda: client.feature_flags)
    keys = sorted(f.get("key") for f in definitions) if isinstance(definitions, list) else definitions
    print(json.dumps({"phase": "loaded"}), flush=True)
    # The runner marks the proxy log after "loaded"; everything below must
    # evaluate without a /flags round trip.
    sys.stdin.readline()
    results = evaluate(client, {"plan": "enterprise"})
    results["load"] = load
    results["definition_keys"] = keys
    client.shutdown()
    return results


def capture_one_scenario(token):
    client = Posthog(token, host=HOST)
    client.capture("ct_retry", distinct_id=USER, properties={"marker": os.environ["CT_MARKER"]}, uuid=os.environ["CT_UUID"])
    flush = guard(lambda: client.flush())
    client.shutdown()
    return {"flush": flush}


def main():
    scenario = sys.argv[1]
    if scenario == "main":
        out = main_scenario()
    elif scenario == "local":
        out = local_scenario()
    elif scenario == "capture":
        out = capture_one_scenario(TOKEN)
    elif scenario == "bad_token":
        out = capture_one_scenario(f"phc_unknown{RUN}")
    else:
        raise SystemExit(f"unknown scenario {scenario}")
    out["sdk_version"] = posthog.VERSION
    print(json.dumps({"phase": "done", "results": out}, default=str), flush=True)


if __name__ == "__main__":
    main()
