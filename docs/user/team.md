# Team

Hoglet has people, not just an owner. Each person has a role in each
organization. No email server is needed: you invite someone by handing them a
one-time link.

## Roles

Roles belong to an organization. Someone can be an owner of one and a member
of another.

| | Owner | Admin | Member |
|---|---|---|---|
| See analytics, persons, events, flags, insights, dashboards, status | yes | yes | yes |
| See the member list | yes | yes | yes |
| Create and change flags, insights, dashboards, shares, forwarding; erase a person; load demo data | yes | yes | no |
| Create projects | yes | yes | no |
| See and revoke pending invites; invite people | yes | yes | no |
| Invite, promote, demote or remove admins and members | yes | yes | no |
| Invite, promote, demote or remove owners | yes | no | no |
| Leave the organization | yes (if not the last owner) | yes | yes |

Rules that always hold:

- An organization always has at least one owner. The last owner cannot be
  demoted, removed, or leave (`409 last_owner`). Make someone else an owner
  first.
- A member is read-only everywhere, including through their own API keys: a
  `write`-scoped key held by a member is still refused (`403`). A key can never
  manage people or create invites; that needs a signed-in session.
- Role changes and removals take effect on the next request. Nothing is cached.

The dashboard disables the buttons a member cannot use and says why on hover.
The server enforces the rules either way.

## Invite someone

Settings, Members, **Invite member**: enter their email and a role, then copy
the link and send it yourself (chat, email, whatever you use). Hoglet does not
send email.

The link looks like `https://analytics.example.com/invite/hgi_...`.

- It is shown once. Hoglet stores only a fingerprint (SHA-256), so a lost link
  cannot be shown again. Use **New link** on the pending invite: it replaces the
  old link with a new one.
- It works once and expires after 7 days. Revoke it any time from the pending
  list; a revoked, used, expired or replaced link all answer the same way
  ("this invite link doesn't work").
- An organization holds at most 100 pending invites and 500 members.
- Treat the link like a password for that person's seat: anyone holding it can
  join with the invited role until it is used. Send it over a channel only the
  invitee can read.

With the API:

```sh
curl -s -b jar -X POST https://analytics.example.com/api/organizations/$ORG/invites \
  -H 'content-type: application/json' -d '{"email":"ann@example.com","role":"member"}'
# {"invite":{...},"token":"hgi_...","path":"/invite/hgi_..."}
```

## Accept an invite

The invitee opens the link.

- **New to this server:** they choose a name and a password (at least 12
  characters) and are signed in as a member of the organization.
- **Already has an account here** (same email): they sign in with their
  existing password instead, and the organization is added to their account.
  The same sign-in throttle as the login form applies.

The email address is fixed by the invite; it cannot be changed on the accept
page.

## Change a role, remove someone

Settings, Members: pick a role in the table, or **Remove** (with a
confirmation). **Leave** is shown on your own row.

Removing someone ends their sessions. When they have no organization left, their
personal API keys are revoked too. If they are still in another organization,
their keys keep working there and nowhere else.

## If the owner is locked out

Hoglet has no email, so there is no "forgot password" link. If you can run
commands on the server, reset the password from the command line:

```sh
sudo systemctl stop hoglet                     # the server must be stopped
sudo -u hoglet env HOGLET_DATA=/var/lib/hoglet \
  hoglet user reset-password --email you@example.com
# New password (at least 12 characters):
sudo systemctl start hoglet
```

It prompts twice without echo, or reads one line from a pipe, or uses
`HOGLET_NEW_PASSWORD` if set (visible in the process environment while it
runs, so prefer the prompt on shared machines). It ends all of that account's
sessions. It refuses to run while a server is using the data directory, so it
cannot collide with a running instance. Docker:

```sh
docker stop hoglet
docker run --rm -it -v hoglet-data:/data -e HOGLET_DATA=/data \
  ghcr.io/<owner>/hoglet:<version> user reset-password --email you@example.com
docker start hoglet
```

`hoglet user list` prints every account with its organizations and roles. It is
read-only and works while the server runs.

```sh
hoglet user list
# you@example.com    you@example.com    Acme (owner)
# ann@example.com    Ann                Acme (member)
```

A reset changes the password only. If the old password may have leaked, also
revoke the account's personal API keys in Settings, API keys, after signing in.
