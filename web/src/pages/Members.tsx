// Settings → Members: who is in the organization, what they may do, and the
// invite links that have not been used yet. Invites need no email server: the
// link is shown once, here, for the inviter to hand over.

import { useForm } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { AppDialog, Confirm } from "@/components/dialogs";
import { CopyButton } from "@/components/copy";
import { DataTable, columnHelper } from "@/components/data-table";
import { Empty, ErrorState, Notice, SkeletonRows } from "@/components/feedback";
import { Icon } from "@/components/icons";
import { CardPad, FormField, Panel } from "@/components/page";
import { toast } from "@/components/toast";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { api, errorMessage, type CreatedInvite, type Invite, type Member, type Role } from "@/lib/api";
import { canEdit, useApp } from "@/lib/context";
import { fmtDate } from "@/lib/format";
import { invitesQuery, membersQuery, qk } from "@/lib/queries";

const ROLES: { value: Role; label: string; hint: string }[] = [
  { value: "owner", label: "Owner", hint: "Everything, including members and other owners." },
  { value: "admin", label: "Admin", hint: "Manage projects, flags, keys and members, except owners." },
  { value: "member", label: "Member", hint: "Read-only: see analytics and resources, change nothing." },
];
const roleLabel = (role: Role) => ROLES.find((r) => r.value === role)?.label ?? role;
const asRole = (value: unknown): Role => (value === "owner" || value === "admin" ? value : "member");

/** Roles `actor` may hand out. Admins cannot create owners. */
const grantable = (actor: Role): Role[] => (actor === "owner" ? ["owner", "admin", "member"] : ["admin", "member"]);
/** Whether `actor` may change or remove someone who holds `target`. */
const mayManage = (actor: Role, target: Role): boolean => actor === "owner" || (actor === "admin" && target !== "owner");

function RoleSelect({ value, options, label, onChange, id }: { value: Role; options: Role[]; label: string; onChange: (role: Role) => void; id?: string }) {
  return (
    <Select value={value} items={ROLES.map((r) => ({ value: r.value, label: r.label }))} onValueChange={(v) => onChange(asRole(v))}>
      <SelectTrigger id={id} size="sm" className="w-28" aria-label={label}>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {ROLES.filter((r) => options.includes(r.value)).map((r) => (
          <SelectItem key={r.value} value={r.value}>
            {r.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

function expiresIn(expiresAt: number): string {
  const seconds = expiresAt - Math.floor(Date.now() / 1000);
  if (seconds <= 0) return "expired";
  const days = Math.floor(seconds / 86_400);
  if (days >= 1) return `in ${days}d`;
  return `in ${Math.max(1, Math.floor(seconds / 3600))}h`;
}

/** The one place the link is visible. It cannot be fetched again; "New link" issues another. */
function InviteLinkDialog({ created, onClose }: { created: CreatedInvite; onClose: () => void }) {
  const url = `${window.location.origin}${created.path}`;
  return (
    <AppDialog title="Send this link" description={`${created.invite.email} joins as ${roleLabel(created.invite.role).toLowerCase()}.`} onClose={onClose} footer={<Button onClick={onClose}>Done</Button>}>
      <div className="flex flex-col gap-3">
        <Notice tone="warn">This link is shown once and can be used once. Anyone who has it can join, so send it only to {created.invite.email}. It expires in 7 days.</Notice>
        <div className="flex items-center gap-2 rounded-lg border bg-muted/40 py-1.5 pr-1.5 pl-3">
          <code className="min-w-0 flex-1 font-mono text-xs break-all" aria-label="Invite link">
            {url}
          </code>
          <CopyButton text={url} />
        </div>
      </div>
    </AppDialog>
  );
}

function InviteDialog({ organizationId, actor, onClose }: { organizationId: string; actor: Role; onClose: () => void }) {
  const queryClient = useQueryClient();
  const [created, setCreated] = useState<CreatedInvite | null>(null);
  const [serverError, setServerError] = useState<string | null>(null);
  const form = useForm({
    defaultValues: { email: "", role: "member" as Role },
    onSubmit: async ({ value }) => {
      setServerError(null);
      try {
        setCreated(await api.createInvite(organizationId, { email: value.email.trim(), role: value.role }));
        await queryClient.invalidateQueries({ queryKey: qk.invites(organizationId) });
      } catch (e) {
        setServerError(errorMessage(e));
      }
    },
  });
  if (created) return <InviteLinkDialog created={created} onClose={onClose} />;
  return (
    <AppDialog
      title="Invite a member"
      description="You get a one-time link to send them. No email is sent from this server."
      onClose={onClose}
      footer={
        <>
          <Button type="button" variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <form.Subscribe selector={(s) => [s.canSubmit, s.isSubmitting] as const}>
            {([canSubmit, isSubmitting]) => (
              <Button type="submit" form="invite-form" disabled={!canSubmit || isSubmitting}>
                {isSubmitting ? "Creating…" : "Create invite link"}
              </Button>
            )}
          </form.Subscribe>
        </>
      }
    >
      <form
        id="invite-form"
        noValidate
        className="flex flex-col gap-4"
        onSubmit={(e) => {
          e.preventDefault();
          e.stopPropagation();
          void form.handleSubmit();
        }}
      >
        <form.Field
          name="email"
          validators={{
            onMount: ({ value }) => (value.trim() ? undefined : "Email is required."),
            onChange: ({ value }) => (/^\S+@\S+\.\S+$/.test(value.trim()) ? undefined : "Enter a valid email address."),
          }}
        >
          {(field) => {
            const shown = field.state.meta.isTouched && field.state.meta.errors.length > 0 ? String(field.state.meta.errors[0]) : serverError;
            return (
              <FormField label="Email" htmlFor="invite-email" error={shown}>
                <Input
                  id="invite-email"
                  type="email"
                  autoFocus
                  autoComplete="off"
                  placeholder="teammate@example.com"
                  value={field.state.value}
                  aria-invalid={!!shown || undefined}
                  onChange={(e) => {
                    setServerError(null);
                    field.handleChange(e.target.value);
                  }}
                  onBlur={field.handleBlur}
                />
              </FormField>
            );
          }}
        </form.Field>
        <form.Field name="role">
          {(field) => (
            <FormField label="Role" htmlFor="invite-role" hint={ROLES.find((r) => r.value === field.state.value)?.hint}>
              <RoleSelect id="invite-role" value={field.state.value} options={grantable(actor)} label="Role" onChange={(r) => field.handleChange(r)} />
            </FormField>
          )}
        </form.Field>
      </form>
    </AppDialog>
  );
}

const memberCol = columnHelper<Member>();
const inviteCol = columnHelper<Invite>();

export function MembersSettings() {
  const { organization, workspace, refreshWorkspace } = useApp();
  const organizationId = organization.id;
  const me = workspace.user.id;
  const actor = organization.role;
  const manager = canEdit(organization);
  const queryClient = useQueryClient();
  const members = useQuery(membersQuery(organizationId));
  const invites = useQuery({ ...invitesQuery(organizationId), enabled: manager });
  const [inviting, setInviting] = useState(false);
  const [removing, setRemoving] = useState<Member | null>(null);
  const [link, setLink] = useState<CreatedInvite | null>(null);

  const refresh = async () => {
    await Promise.all([queryClient.invalidateQueries({ queryKey: qk.members(organizationId) }), queryClient.invalidateQueries({ queryKey: qk.invites(organizationId) })]);
  };

  const changeRole = useMutation({
    mutationFn: ({ member, role }: { member: Member; role: Role }) => api.updateMember(organizationId, member.user_id, role),
    onSuccess: async (updated) => {
      toast(`${updated.name} is now ${roleLabel(updated.role).toLowerCase()}`);
      await refresh();
      if (updated.user_id === me) await refreshWorkspace();
    },
    onError: (e) => toast(errorMessage(e), true),
  });
  const remove = useMutation({
    mutationFn: (member: Member) => api.removeMember(organizationId, member.user_id),
    onSuccess: async (_, member) => {
      toast(member.user_id === me ? "You left the organization" : `${member.name} was removed`);
      await refresh();
      if (member.user_id === me) await refreshWorkspace();
    },
  });
  const revoke = useMutation({
    mutationFn: (invite: Invite) => api.revokeInvite(organizationId, invite.id),
    onSuccess: async () => {
      toast("Invite revoked");
      await refresh();
    },
    onError: (e) => toast(errorMessage(e), true),
  });
  const reissue = useMutation({
    mutationFn: (invite: Invite) => api.createInvite(organizationId, { email: invite.email, role: invite.role }),
    onSuccess: async (created) => {
      setLink(created);
      await refresh();
    },
    onError: (e) => toast(errorMessage(e), true),
  });

  const nameOf = (userId: string) => members.data?.find((m) => m.user_id === userId)?.name;

  const memberColumns = useMemo(
    () => [
      memberCol.accessor("name", {
        header: "Name",
        cell: ({ row }) => (
          <span className="inline-flex items-center gap-2 font-semibold">
            {row.original.name}
            {row.original.user_id === me && <Badge variant="secondary">you</Badge>}
          </span>
        ),
      }),
      memberCol.accessor("email", { header: "Email", cell: (c) => <span className="text-muted-foreground">{c.getValue()}</span> }),
      memberCol.accessor("role", {
        header: "Role",
        cell: ({ row }) => {
          const m = row.original;
          return manager && mayManage(actor, m.role) ? (
            <RoleSelect value={m.role} options={grantable(actor)} label={`Role of ${m.name}`} onChange={(role) => role !== m.role && changeRole.mutate({ member: m, role })} />
          ) : (
            <Badge variant={m.role === "member" ? "secondary" : "default"}>{roleLabel(m.role)}</Badge>
          );
        },
      }),
      memberCol.accessor("joined_at", { header: "Joined", cell: (c) => <span className="text-muted-foreground">{fmtDate(c.getValue())}</span> }),
      memberCol.display({
        id: "actions",
        header: () => <span className="sr-only">Actions</span>,
        cell: ({ row }) => {
          const m = row.original;
          const self = m.user_id === me;
          if (!self && !(manager && mayManage(actor, m.role))) return null;
          return (
            <Button variant="ghost" size="sm" className="text-destructive hover:text-destructive" aria-label={self ? "Leave organization" : `Remove ${m.name}`} onClick={() => setRemoving(m)}>
              {self ? "Leave" : "Remove"}
            </Button>
          );
        },
        meta: { align: "right" },
      }),
    ],
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [me, actor, manager],
  );

  const inviteColumns = useMemo(
    () => [
      inviteCol.accessor("email", { header: "Email", cell: (c) => <span className="font-semibold">{c.getValue()}</span> }),
      inviteCol.accessor("role", { header: "Role", cell: (c) => <Badge variant="secondary">{roleLabel(c.getValue())}</Badge> }),
      inviteCol.accessor((i) => nameOf(i.created_by) ?? "–", { id: "by", header: "Invited by", cell: (c) => <span className="text-muted-foreground">{c.getValue()}</span> }),
      inviteCol.accessor("expires_at", { header: "Expires", cell: (c) => <span className="text-muted-foreground">{expiresIn(c.getValue())}</span> }),
      inviteCol.display({
        id: "actions",
        header: () => <span className="sr-only">Actions</span>,
        cell: ({ row }) =>
          mayManage(actor, row.original.role) ? (
            <span className="inline-flex gap-1">
              <Button variant="ghost" size="sm" aria-label={`New link for ${row.original.email}`} disabled={reissue.isPending} onClick={() => reissue.mutate(row.original)}>
                New link
              </Button>
              <Button variant="ghost" size="sm" className="text-destructive hover:text-destructive" aria-label={`Revoke invite for ${row.original.email}`} onClick={() => revoke.mutate(row.original)}>
                Revoke
              </Button>
            </span>
          ) : null,
        meta: { align: "right" },
      }),
    ],
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [actor, members.data, reissue.isPending],
  );

  return (
    <div className="flex flex-col gap-4">
      <Panel>
        <CardPad className="flex flex-wrap items-center gap-3">
          <div className="min-w-0 flex-1">
            <h2>Members of {organization.name}</h2>
            <p className="mt-0.5 text-muted-foreground">
              Owners and admins manage the team and the project; members can look but not change anything. You are {{ owner: "an owner", admin: "an admin", member: "a member" }[actor]}.
            </p>
          </div>
          {manager && (
            <Button onClick={() => setInviting(true)}>
              <Icon name="plus" size={14} /> Invite member
            </Button>
          )}
        </CardPad>
      </Panel>
      {!manager && <Notice tone="info">You have read-only access. Ask an owner or admin to change roles or invite people.</Notice>}
      <Panel>
        {members.error && !members.data ? (
          <ErrorState error={members.error} retry={() => void members.refetch()} />
        ) : members.isPending ? (
          <SkeletonRows rows={3} />
        ) : (
          <DataTable label="Members" columns={memberColumns} data={members.data ?? []} getRowId={(m) => m.user_id} />
        )}
      </Panel>
      {manager && (
        <>
          <h2 className="mt-2">Pending invites</h2>
          <Panel>
            {invites.error && !invites.data ? (
              <ErrorState error={invites.error} retry={() => void invites.refetch()} />
            ) : invites.isPending ? (
              <SkeletonRows rows={2} />
            ) : invites.data && invites.data.length === 0 ? (
              <Empty icon="users" title="No pending invites">
                Invite a teammate to get a one-time link for them. Links expire after 7 days.
              </Empty>
            ) : (
              <DataTable label="Pending invites" columns={inviteColumns} data={invites.data ?? []} getRowId={(i) => i.id} />
            )}
          </Panel>
          <p className="text-xs text-muted-foreground">Links are not stored, only a fingerprint of them, so an old link cannot be shown again: use “New link” to replace it.</p>
        </>
      )}
      {inviting && <InviteDialog organizationId={organizationId} actor={actor} onClose={() => setInviting(false)} />}
      {link && <InviteLinkDialog created={link} onClose={() => setLink(null)} />}
      {removing && (
        <Confirm
          title={removing.user_id === me ? "Leave this organization?" : `Remove ${removing.name}?`}
          body={
            removing.user_id === me
              ? "You lose access to its projects. Someone with the owner or admin role has to invite you back."
              : `${removing.email} loses access to this organization's projects immediately, and their sign-ins and personal API keys stop working.`
          }
          confirmLabel={removing.user_id === me ? "Leave" : "Remove member"}
          danger
          onClose={() => setRemoving(null)}
          onConfirm={() => remove.mutateAsync(removing)}
        />
      )}
    </div>
  );
}
