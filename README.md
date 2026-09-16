# solana-savings-circle

An Anchor program for a rotating savings circle (a digital ROSCA / chit fund): a fixed group of members each contribute a fixed amount of an SPL token every round, and each round the whole pot goes to one member — picked at random from whoever hasn't won yet — until everyone has been paid out exactly once.

## Why

A rotating savings circle lets a group pool money without a bank: each round everyone contributes the same amount, one person gets the lump sum, and by the end of a full cycle everyone has both paid in and received the pot exactly once. Random draw (rather than a fixed queue) means no one has to negotiate who goes first.

## What's new here vs. other repos in this org

- **Round-based state machine** — state changes over repeated rounds (who's paid this round, who's already won), not a one-shot lock/unlock or single pooled withdrawal.
- **`remaining_accounts` + runtime validation** — the payout winner is only known *after* on-chain randomness picks them mid-instruction, so their token account can't be a fixed, typed field in the `Accounts` struct. It's looked up at runtime from a list of every member's token account and checked against each member's expected Associated Token Account (ATA) address before any transfer happens.
- **On-chain pseudo-randomness** via the current `Clock::get()?.slot` — see the security note below.

## Instructions

| Instruction | Signer | Description |
|---|---|---|
| `create_circle(circle_id, contribution_amount, member_count)` | creator | Creates a `SavingsCircle` PDA and its token vault. `member_count` must be 2..=10. |
| `join_circle()` | member | Joins an open circle; creates the member's ATA if needed. Once the circle fills up, it auto-starts (`current_round = 1`). |
| `contribute()` | member | Pays `contribution_amount` into the vault for the current round. Once every member has paid, a winner is auto-selected among members who haven't won yet, and the full pot is paid out to them in the same instruction. |
| `close_circle()` | creator | Closes the vault and circle accounts once every round has paid out, reclaiming rent. |

## Accounts

**`SavingsCircle`** — PDA at `["circle", creator, circle_id]`
- `creator: Pubkey`, `circle_id: u64`, `mint: Pubkey`, `contribution_amount: u64`
- `member_count: u8`, `joined_count: u8`, `members: [Pubkey; 10]`
- `current_round: u8` (0 before full, 1..=member_count while running, `member_count + 1` once complete)
- `contributed_mask: u16` (who's paid this round), `paid_out_mask: u16` (who's already won)
- `started: bool`, `bump: u8`

**Vault** — a plain SPL `TokenAccount` PDA at `["vault", circle]`, with the `SavingsCircle` PDA itself as token authority (it signs outgoing transfers with its own seeds).

## Security note on randomness

The winner each round is chosen using `Clock::get()?.slot` as a pseudo-random seed. This is **not** manipulation-proof — a validator has some influence over which slot a transaction lands in. It's fine for a learning project, but a real deployment handling real money should use a verifiable randomness source instead (e.g. Switchboard VRF).

## Building and testing

Requires `solana-cli`, `anchor-cli`, and Rust already installed. This machine needed `platform-tools` v1.57 to avoid an `edition2024` build error (same issue as `solana-charity-donations` — see that repo's README for the one-time fix).

```bash
anchor build --no-idl -- --tools-version v1.57
anchor idl build -o target/idl/solana_savings_circle.json -t target/types/solana_savings_circle.ts
anchor test --skip-build --no-idl
```

`cargo clippy` (run from `programs/solana-savings-circle`) is clean.

## CLI client

`cli/` is a Rust CLI (`circle-cli`, built with `anchor-client` + `clap`) covering every instruction, plus a `show` command to read a circle's on-chain state. Defaults to a local validator (`http://127.0.0.1:8899` / `ws://127.0.0.1:8900`) — override with `--url`/`--ws-url` for devnet or mainnet.

```bash
cargo build -p circle-cli
BIN=./target/debug/circle-cli

# local validator + program deploy + a test SPL mint:
solana-test-validator --reset --quiet &
solana program deploy target/deploy/solana_savings_circle.so \
  --program-id target/deploy/solana_savings_circle-keypair.json
spl-token create-token --fee-payer ~/creator.json --mint-authority ~/creator.json --decimals 6

$BIN create-circle --keypair ~/creator.json --circle-id 1 --mint <MINT> \
  --contribution-amount 10000000 --member-count 3

$BIN join-circle --keypair ~/member1.json --creator <CREATOR_PUBKEY> --circle-id 1
# ...repeat join-circle for every member; auto-starts once full

# mint each member enough tokens for all rounds (member_count * contribution_amount),
# then each member calls contribute() once per round:
$BIN contribute --keypair ~/member1.json --creator <CREATOR_PUBKEY> --circle-id 1

$BIN show --creator <CREATOR_PUBKEY> --circle-id 1

$BIN close-circle --keypair ~/creator.json --circle-id 1
```

Run `$BIN --help` or `$BIN <command> --help` for the full flag list.
