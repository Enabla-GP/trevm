# Canonical system effects in the Enabla fork

`0.43.2-enabla.1` corrects EIP-4895 body withdrawals and the failure behavior of
EIP-7002/EIP-7251 post-block request helpers. It retains the explicit
[`HeaderWithBlobParams` contract](header-blob-params.md) from `0.43.1-enabla.1`.

## Body withdrawals

`Withdrawal.amount` is the wire amount in Gwei. `apply_withdrawals` converts it
using Alloy's `amount_wei()` and adds it to the recipient's wei balance. Raw
amounts 1 and 2 to an account initially holding 7 wei yield 3,000,000,007 wei.
Amounts must not be pre-multiplied by callers.

Repeated recipients reuse one pending Account, preserving its account metadata
and untouched storage. The existing saturating balance arithmetic is retained.
The helper commits the accumulated changes once, after all required account
reads succeed. A later account-read failure returns the original database
error without committing any staged credits.

## Prague request post-ops

`apply_eip7002` and `apply_eip7251` return request bytes only for a successful
system call. Revert and Halt return `EVMError::Custom` identifying the operation
and failure class. Revert output is never request output; Halt does not panic.
Ordinary database/interpreter errors propagate as before. Pre-Prague calls
remain no-ops. Generic system execution is not changed to reject all Revert or
Halt results: the beacon/history pre-op rules differ.

The result change is intentional. Callers must handle Err by rejecting the
block execution; they must not parse revert bytes, catch a halt panic, or mark
an accepted prefix as complete. The fork patch version changes accordingly.

## Caller-owned canonical lifecycle

The caller supplies chain-resolved fork/blob parameters and complete child
context while opening parent post-state. The selected Ethereum lifecycle is
history/beacon pre-ops, sequential user transactions, deposit/withdrawal/
consolidation request processing, then body withdrawals. Empty blocks still
require applicable protocol operations.

This release does not alter helper code-insertion behavior. Before invoking
insertion-capable helpers, canonical callers must apply the specification's
presence rules: absent history/beacon code is a silent no-op; absent Prague
request-predeploy code invalidates the block. The caller also validates body
and header commitments, request byte lengths/type order/hash, configured
deposit address and fallible log decoding, aggregate block/blob gas limits,
and ordered receipts. Receipt equality alone does not establish withdrawal
post-state or a trie root.

On any protocol/database/interpreter error, the canonical caller must discard
the request-local EVM and overlay. Generic temporary environment restoration
on an error is tracked separately in [ENA-2858](https://linear.app/enabla-gp/issue/ENA-2858/trevm-restore-temporary-system-call-environment-after-an-interpreter); this release does not repair
it or authorize retaining that EVM. Unsupported eras and unconfigured chains
must fail explicitly. No resolver, producer, worker supervision, root consumer
pin, or live-readiness claim is included in this fork change.

Primary protocol sources: [EIP-4895](https://eips.ethereum.org/EIPS/eip-4895),
[EIP-7002](https://eips.ethereum.org/EIPS/eip-7002), and
[EIP-7251](https://eips.ethereum.org/EIPS/eip-7251).


## System beneficiary effects (0.43.3-enabla.1)

The shared EIP-4788/7002/7251 execution helper uses revm's native inspected
system-call path and its protocol-aware gas budget, including the pinned
implementation's reservoir allowance. System calls have no ordinary transaction
validation, intrinsic gas, caller gas deduction or beneficiary gas reward.
Inspector callbacks remain enabled.

Cleanup must preserve legitimate contract storage and account effects even when
the block beneficiary is a system target or another affected address. It must
not remove the entire beneficiary account to suppress fees, and it must not
synthesize a zero-value fee touch at an otherwise untouched empty beneficiary.
Actual deployed-code regressions cover beacon timestamp/root storage and both
request queue resets with their target as beneficiary; draining cannot repeat
the same request. Positive controls cover unrelated balance/nonce and absence
of artificial empty-account touches.

The existing successful temporary-environment restoration and system-caller
cleanup remain. Backend/interpreter errors still leave before successful
cleanup; generic error restoration is separately tracked by ENA-2858 and is not
included in this correction. The canonical caller drops the entire EVM/overlay
on such errors. EIP-7002/7251 strict Success output handling remains unchanged.
