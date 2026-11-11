# Running

## Run

Offline (default — **no server required**):

```bash
cargo run
# same as:
cargo run -- --mode offline

# Skip select screen:
cargo run -- --hero vanguard
cargo run -- --hero skirmisher
cargo run -- --hero arcanist
```

Release build:

```bash
cargo run --release
```

### Multiplayer (optional UDP 1v1)

Host (authoritative listen server + local Radiant hero):

```bash
cargo run -- --mode host --addr 0.0.0.0:7777
```

Client (connects as Dire; sends orders, receives hero snapshots):

```bash
cargo run -- --mode client --addr 127.0.0.1:7777
```

The HUD shows connection status. Host sim runs combat / creeps / AI; clients apply hero snapshots (~20 Hz). Spells/items are host-authoritative for now (clients move/attack via the network).
