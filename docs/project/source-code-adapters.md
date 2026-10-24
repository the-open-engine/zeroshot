# Source-code adapters

Local folders, Git transport, and forge delivery are separate concerns. A local graph without
delivery needs no source identity. Contained runs require an exact Git source. The existing push,
PR, and merge nodes keep their receipt, repair, cancellation, and recovery behavior across providers.

| Operation                                    | Required implementation  |
| -------------------------------------------- | ------------------------ |
| Checkout and push over HTTPS or SSH          | Shared Git transport     |
| Review creation, checks, feedback, and merge | Registered forge adapter |

GitLab, Forgejo, and other Git hosts can use checkout and push now. Their forge APIs need adapters
for PR and merge behavior. GitHub is the maintained forge implementation.

## Ownership and registration

`zeroshot/src/source_code/provider/` contains effect-free contracts and
`SourceCodeProviderRegistry`. Provider, repository, and review identities are bounded opaque values;
GitHub grammar and policy belong to its implementation.

Maintained implementations live in `zeroshot/src/source_code/adapters/core/`. Contributions live in
`zeroshot/src/source_code/adapters/contributed/` and register through that folder's `register` hook.
The same registry proves required capabilities during preflight and returns delivery authority
during composition. Metadata-only registration must perform no filesystem, process, credential, or
network effects. The provider's pure `repository_from_remote` hook identifies its forge from a
credential-free Git locator text after the run boundary validates the URL. Contracts stay independent
of protocol and process implementations. The same registry rejects ambiguous matches; both local and named CLI
source selection use it. Runtime registration receives the selected source and scoped shared Git transport.

Implement `SourceCodeProvider` for descriptor and source operations, and `SourceDeliveryAuthority`
for the existing delivery workflow. A forge owns its credential field, repository and review
grammar, checks, policy, and authoritative merge result. Git effects reuse the shared transport.
A custom API token never acquires GitHub Git authentication authority. Provider credentials are
available only through the admitted delivery connection or private source capability.

Adding a forge needs no new delivery node family or closed forge enum. Embedders can register adapters in
`SourceCodeProviderRegistry` and use registry-aware admission. The contribution acceptance test
admits an opaque nested repository and review ID, uses its own token field and credential refresh,
and drives the existing PR and merge nodes through the supervisor and ledger.

## Required behavior

An adapter must observe remote identity before effects, preserve the reviewed candidate SHA and
published ancestry, and reconcile confirmed success after a lost response. It must distinguish
transport errors, authentication refusal, provider policy/schema errors, and candidate repair.
Review creation may allocate a new opaque ID. Merge receipts record the observed integrated revision;
requests cannot predict it.

Source credential values stay out of source records, receipts, profiles, and history.
Shared transport owns bounded capture, redaction, private SSH files, cancellation, and
process cleanup. Recovery preserves the original delivery branch and review identity while obtaining
fresh authorized values.

Run the shared provider contract tests and existing delivery tests, plus the contributed adapter's
own tests. Include unknown/unsupported capabilities, opaque identities, credential rotation,
lost responses, remote branch movement, cancellation, and failed cleanup.

The production registration hook is compiled into the maintained local and hosted factories.
An embedder may provide its own registry to admission and use that registry's delivery authority
when composing a candidate or allocator. Built-in placements use the contributed registration hook.

The registry's coordinated delivery workflow obtains a scoped adapter through
SourceCodeProvider.delivery_authority. Its descriptor advertises the operations that adapter
supports. The older inspect_operation/operate contract remains available for callers of atomic
operations; maintained adapters currently return an explicit unavailable error through that seam.
Forge credential selection has one authority: SourceDeliveryAuthority.forge_credential_field.

Conversation comments are a separate operation exposed through the same registry's optional
comment authority. Pure URL identification and capability admission do not require a Git source.
The native publisher owns bounded create/reconciliation and a durable lineage publication store;
the forge supplies actor, comment identity, paginated reads, creation, and exact review-scope checks.
Comment authority does not acquire Git transport access or a model session.
