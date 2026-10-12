# carnet-server

The sync server for [Carnet](../../README.md). It orders, stores and passes on the sealed records of your notes, and never holds a key that opens them: a copy of its data shows only noise and public keys. (The one key it does keep signs its short-lived links to pictures.) It also serves Carnet's web app.

It is one binary, with SQLite and a folder of files: nothing else to run. [docs/SYNC.md](../../docs/SYNC.md) describes the design, and its §7 what a server can and cannot see.

## Run it

The released images are on Docker Hub as `qxuken/carnet-server` and on GitHub's registry as `ghcr.io/qxuken/carnet-server`, for amd64 and arm64. Each release is tagged with its version (`0.1.1`), its minor version (`0.1`) and `latest`.

```bash
docker run -d --name carnet -v carnet:/data -p 8080:8080 -e CARNET_ORIGIN=https://notes.example.com qxuken/carnet-server
```

Or with Docker Compose:

```yaml
services:
  carnet:
    image: qxuken/carnet-server:latest
    restart: unless-stopped
    ports:
      - "127.0.0.1:8080:8080"   # a proxy in front does TLS (below)
    environment:
      CARNET_ORIGIN: https://notes.example.com
      CARNET_TRUSTED_PROXY: 172.16.0.0/12
    volumes:
      - carnet:/data
volumes:
  carnet:
```

The image runs as an unprivileged user, keeps everything in the `/data` volume, listens on port 8080, serves the web app at `/`, and checks its own health (so `docker ps` says `healthy`).

On a start with no accounts, it prints an invite code for the first account, the owner's:

```
carnet-server: no accounts yet. Invite code for the first account (owner):
    K7QF-2M9D-XW4H-P3TA   (single use, good for 7 days)
```

`docker logs carnet` shows it. It prints a new one on every start until an account exists. Type the server's address and this code in Carnet's **Settings → Sync** to make the account.

### Building it yourself

The container, from the repository's root:

```bash
docker build -f packaging/server/Dockerfile -t carnet-server .
```

Or the binary, with [Rust](https://rustup.rs):

```bash
cargo run --release -p carnet-server -- --data ./data --listen 127.0.0.1:8080 --origin https://notes.example.com
```

A binary built this way doesn't include the web app; [The web app](#the-web-app) below says how to build and serve it.

### Options

| flag | variable | default | |
|---|---|---|---|
| `--data DIR` | `CARNET_DATA` | `./data` | where the databases are kept; the container's is `/data` |
| `--listen ADDR` | `CARNET_LISTEN` | `127.0.0.1:8080` | the container's is `0.0.0.0:8080` |
| `--origin URL` | `CARNET_ORIGIN` | `http://` + the listen address | the URL devices use to reach it |
| `--web DIR` | `CARNET_WEB` | none | the web app's build (`web/dist`), served at `/`; the container has it |
| `--metrics ADDR` | `CARNET_METRICS` | none | serve `/metrics` there, a listener of its own (see below) |
| `--trusted-proxy LIST` | `CARNET_TRUSTED_PROXY` | none | the proxies in front (`127.0.0.1`, `172.16.0.0/12`), comma separated, whose word on who a request is from is taken (see below) |
| `--passwords` | `CARNET_PASSWORDS=1` | off | let devices set an email and password, and sign in with them (OPAQUE: the password never reaches the server). Off, codes and the recovery words are the ways in |
| `--compact-after N` | `CARNET_COMPACT_AFTER` | `200` | a note kept as this many records is asked to be compacted into one |

`carnet-server --help` lists them all, with the commands below.

**The origin matters.** A device signs its login for the server's address, so a server that isn't the one it meant can't pass the login on. Behind a proxy, set the public `https://…` address. A device whose address differs is turned away.

## The web app

Open the server's address in a browser and type a pairing code, or scan the QR code the app shows beside it with a phone's camera: the browser becomes one of your devices. It reads, searches and edits your notes, ticks their checkboxes, shows a note's history, lists and removes devices, and keeps working offline (install it from the browser's menu, or Share → Add to Home Screen on an iPhone). It decrypts in the browser, like the app. The container serves it. To build it yourself, with Node, the `brotli` command, Rust's `wasm32-unknown-unknown` target and `wasm-bindgen-cli` 0.2.129:

```bash
cd web && npm ci && npm run build
```

```bash
cargo run --release -p carnet-server -- --web web/dist
```

## What it keeps

```
data/carnet.db                the directory: accounts, invites
data/accounts/<account>.db    one account's devices and sealed records
data/blobs/<account>/<id>     one account's pictures, sealed, a file each
data/blobs/<account>/<id>.drop  a picture no note shows: when it was said (gone 30 days on)
data/spool/                   pictures coming in, until whole (emptied on start)
data/links.key                what signs the links to pictures (never leaves)
data/opaque.key               with --passwords: the server's OPAQUE key (never leaves; keep it in backups)
```

Accounts never meet, so neither do their files. A query in one account's database can't reach another's. One account's writes never wait on another's. Deleting an account deletes its file, and moving one copies its file.

All of it is readable by the server's own user alone: the folders `700`, the files `600`. A data folder from an older server that others could read is closed when it starts, and the log says so once.

## Invites, pairing, recovery

An invite makes a new account, a person with their own notes. Adding a device to an account is pairing, done from a device that's already in: it shows a code like `47-381095`, and the new device types it. The server relays the two sides' messages, and can't read or change what passes between them. When no device is left, the account's 12-word recovery code lets a new one in.

The first account is the owner's: its devices make more invites, in Settings → Sync (or Settings in the browser) → **Invite someone**. The owner sees the invites not used yet, and can take one back, and sees the other accounts, and can remove one with everything it keeps here. From the shell, for an owner who has lost every device:

```bash
carnet-server invite --data ./data
```

```bash
docker exec carnet carnet-server invite
```

Each is single-use and good for a week. Only its hash is kept: an invite made in the app never reaches the server at all, only its hash does.

## TLS: a proxy in front

The server speaks plain HTTP; the proxy does TLS. The sync socket is `/v1/sync`, a WebSocket.

**nginx + certbot:**

```nginx
server {
    server_name notes.example.com;
    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_read_timeout 120s;
        client_max_body_size 40m;  # a picture, put at its link
    }
    listen 443 ssl; # managed by certbot
}
```

**Cloudflare:** a proxied DNS record to the server, or a Tunnel (`cloudflared tunnel --url http://localhost:8080`) for a machine with no open port. Cloudflare ends TLS, which is harmless here: all it sees is sealed records. It caps WebSocket messages, and the server's frames stay under 1 MiB; a picture, at most 32 MiB, is well under its upload limit.

**Caddy:**

```
notes.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

**Tell it which proxy is yours.** The server limits how much one address (an IPv6 address's whole /64) may ask of it: sockets opened (30 at once, then one every 2 seconds, at most 64 open), and tries that guess at something or cost it work (a login that fails, an invite used, a pairing joined, a recovery: 10 at once, then one every 6 seconds). A login that succeeds is never counted, so your own devices are never shut out. Behind a proxy, every request comes from the proxy's address, so name it, and the server counts by the address it says each request is for (`CF-Connecting-IP`, else `X-Forwarded-For` read from the right). It believes those headers from the proxies named, and nobody else:

```bash
carnet-server --trusted-proxy 127.0.0.1          # nginx or Caddy on the same machine
carnet-server --trusted-proxy 172.16.0.0/12      # a proxy in another container
```

A socket past the limit is refused with `429`; a try past it, with "too many tries from this address". `carnet_limited_total` in `/metrics` counts both.

**Time limits.** A request's headers must come within 30 seconds, and a connection idle between requests is closed after as long. A request is answered within 30 seconds or gets `408`; a picture's upload or download by its link has 10 minutes. A socket, once open, keeps its own limits (30 seconds to sign in, 5 minutes with nothing heard). So a proxy's own timeouts can be generous (`proxy_read_timeout` above).

**Only its own pages open a socket.** A browser says which page opens a socket (`Origin`), and the server refuses one from another site's page (`403`): the web app is at `--origin`, and the apps send none. So set `--origin` to the address the browser shows. On your own machine (`--origin http://127.0.0.1:…`), a page at `localhost` on any port is taken too, as for the web app's development server.

## Is it well?

`GET /v1/health` answers `200 {"ok":true}` while the server's hub does, and `503` if it doesn't within five seconds: for an uptime monitor, or a load balancer. The container checks it itself (`HEALTHCHECK`, so `docker ps` says `healthy`), through `carnet-server health`, which asks the server on this machine and exits 0 or 1.

For more, give it a metrics address. `/metrics` is served there in Prometheus' text format, on a listener of its own, so it stays off the public address: keep it on `127.0.0.1`, or a private network, and point Prometheus (or Grafana Agent, or anything that reads the format) at it.

```bash
carnet-server --data ./data --metrics 127.0.0.1:9090
```

```bash
docker run -d --name carnet -v carnet:/data -p 8080:8080 -p 127.0.0.1:9090:9090 -e CARNET_METRICS=0.0.0.0:9090 qxuken/carnet-server
```

They're counts, never what anything held: the server has nothing else to give.

| | |
|---|---|
| `carnet_storage_failures_total` | writes to the databases that failed: the one to alert on (a full disk, a broken file) |
| `carnet_limited_total{what}` | sockets refused (`sockets`) and sockets turned away (`tries`) for asking too much from one address |
| `carnet_sockets_closed_total{reason}` | sockets the server closed for its limits: too much waiting for one that isn't reading (`budget`), too long to sign in (`login_timeout`), nothing heard for 5 minutes (`idle`) |
| `carnet_hub_queue_seconds` | how long the scrape waited for the hub, which takes every socket's frames in turn: its lag |
| `carnet_hub_handle_seconds` | a histogram of how long the hub took over each frame |
| `carnet_sockets`, `carnet_sockets_logged_in` | sockets open, and of them devices logged in |
| `carnet_sockets_by_protocol{protocol}` | devices logged in, by the protocol their app speaks: whether an old one can be let go (docs/SYNC.md §14) |
| `carnet_sockets_by_app{app,version}`, `carnet_logins_by_app_total{app,version}` | devices logged in now, and logins since start, by the app (`desktop-macos`, `web`, `web-installed`…) and its version, as the app says in its address; 64 of them at most, the rest `app="other"` |
| `carnet_logins_total{result}`, `carnet_denied_total` | logins let in or refused, and sockets turned away for any reason |
| `carnet_enrollments_total{result}` | invites redeemed or refused |
| `carnet_records_total`, `carnet_record_bytes_total`, `carnet_batches_refused_total` | sealed records kept, and batches refused |
| `carnet_blobs_total`, `carnet_blob_bytes_total` | sealed pictures kept |
| `carnet_blob_links_total`, `carnet_blob_requests_total{way}` | links given to put or fetch a picture, and pictures fetched and put by them |
| `carnet_blobs_dropped_total{step}` | pictures no note shows any more: `marked` when a device says so, `gone` 30 days on |
| `carnet_snapshots_total` | notes compacted into one record |
| `carnet_frames_received_total`, `carnet_frames_sent_total`, `carnet_received_bytes_total`, `carnet_sent_bytes_total` | traffic |
| `carnet_accounts`, `carnet_data_bytes` | accounts, and what the databases take on disk |
| `carnet_pairings_open`, `carnet_pairings_total`, `carnet_recoveries_total` | pairings |
| `carnet_build_info{version}`, `process_start_time_seconds` | the version, and when it started |

The disk's free space isn't among them: the machine's own exporter (`node_exporter`) has it.

## Backup

```bash
carnet-server backup ./carnet-backup --data ./data
```

It copies every database into a new folder laid out as `data/` is, consistent while the server runs (`VACUUM INTO`), and the pictures with them (each a file written whole and never changed). To restore, point `--data` at the copy. The databases and pictures are all sealed or public, so the copy can go anywhere: another disk, a bucket, a friend's machine.

The keys are not copied. The link key doesn't matter: a restored server makes a new one, and links last minutes anyway. But with `--passwords`, every password depends on `data/opaque.key`: copy it beside the backup yourself, and keep that copy as safe as the server.

With Docker, back up inside the container and copy the result out:

```bash
docker exec carnet carnet-server backup /tmp/carnet-backup
docker cp carnet:/tmp/carnet-backup ./carnet-backup
docker exec carnet rm -rf /tmp/carnet-backup
```

## Upgrading

Back up, then start the new version on the same data: `docker pull qxuken/carnet-server` and recreate the container, or replace the binary and restart it. A newer server brings an older data folder (or Postgres database) up to date on its first start; an older server refuses data a newer one has written, so going back means restoring the backup.

Devices keep what they write while the server is down and send it once it answers again. Apps and servers of different versions tell each other which protocol they speak, and `GET /v1/info` says which apps a server still takes ([docs/SYNC.md](../../docs/SYNC.md) §14).

## Postgres and a bucket, for a big server

A server of one's own needs neither: SQLite and files carry thousands of accounts on one machine. Past that, the server can keep its records in Postgres and its pictures in an S3-compatible bucket (AWS S3, Cloudflare R2, Hetzner, MinIO). Both are built in only when asked, and the released images are built without them:

```bash
cargo build --release -p carnet-server --features postgres,s3
```

| flag | variable | |
|---|---|---|
| `--postgres URL` | `CARNET_POSTGRES` | the records in Postgres: `postgres://user:password@host/db`. Add `?sslmode=require` for a database elsewhere (TLS with the web's roots); `sslmode=disable` for one on the same machine or a private network |
| `--s3 URL` | `CARNET_S3` | the pictures in a bucket: `https://host/bucket`, or `https://host/bucket/folder` to keep to a folder of it |
| `--s3-region R` | `CARNET_S3_REGION` | the bucket's region (`us-east-1` by default; R2's is `auto`) |
| `--s3-virtual-host` | `CARNET_S3_VIRTUAL_HOST=1` | the bucket as the host's first name (`bucket.host`), not the path's |
| | `CARNET_S3_ACCESS_KEY`, `CARNET_S3_SECRET_KEY` | the bucket's keys (or AWS's own `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`): only from the environment, never a flag |

- **Postgres.** Every account in one schema, each row naming its account, gone with it. The tables are made on the first start and brought up to date on later ones; a database from a newer server is refused. The server keeps one connection, and makes it again if it drops. The URL holds a password: give it by the variable, not the flag, so `ps` doesn't show it.
- **The bucket.** A device puts and fetches a picture straight to and from the bucket, by a link signed here for five minutes: the bytes never pass through the server, and `/v1/blobs/…` serves nothing. A picture no note shows is marked by an object beside it (`drop/<account>/<id>`), and swept 30 days on, as with files.
  - **CORS** must let the web app `PUT` and `GET` from the server's origin. For AWS (`aws s3api put-bucket-cors --bucket B --cors-configuration file://cors.json`):
    ```json
    {"CORSRules": [{"AllowedOrigins": ["https://notes.example.com"], "AllowedMethods": ["GET", "PUT"], "AllowedHeaders": ["content-type"], "MaxAgeSeconds": 3600}]}
    ```
  - **A put's size.** A device says how long each picture is, sealed, when it offers it, and its `PUT` link is signed with that `Content-Length`: the bucket refuses a put of any other length, so nothing past 32 MiB, or past what was offered, gets in. Within its five minutes a link may put its picture again, over the first: harmless (only the account's own device has the link, and a picture that isn't the one its id names opens on no device), so a conditional put isn't asked for, and CORS needs no more headers.
- **What stays in the data folder:** `opaque.key` (every password depends on it) and `links.key`. Keep `--data` pointing at the folder, and keep it in backups. Postgres holds no secret.
- **Backup** is the database's and the bucket's own: `pg_dump` (or the host's snapshots) and the bucket's versioning or replication. `carnet-server backup` copies SQLite files only.

## Moving to Postgres and a bucket

`carnet-server migrate` copies everything in the data folder to Postgres and the bucket: accounts, devices, the keys sealed to them, records and bases (those set aside too), the log, invites, recovery codes, passwords, passkeys, rooms removed, pictures (also any still in an account's database) and their marks. It:

- runs only with the server stopped: the files are locked while it reads them, and a folder a server has open is refused. The pictures, most of the bytes, can go first while it still runs (`--pictures-first`), so the stop lasts only as long as the records take;
- never writes to the folder, so going back is starting the old server on it;
- can be run again: it skips what's copied (an account the same on both sides, a picture the bucket has with the same size and MD5), copies an account again if it changed, and finishes what a run cut short. Each account goes in one transaction, whole or not at all;
- checks everything after: each account read back from both sides and compared row by row (SHA-256 over its rows), the invites, each picture's size and MD5, the marks. Any difference fails it;
- never copies over a database or bucket a server has run on: from then on they hold what the folder doesn't.

### The switch, step by step

1. **Get ready, while the old server runs.** Build with `--features postgres,s3`. Make an empty Postgres database and a user for it, and the bucket with its CORS (above). Put the URL and keys in the environment:
   ```bash
   export CARNET_POSTGRES='postgres://carnet:…@db.internal/carnet?sslmode=require'
   export CARNET_S3_ACCESS_KEY=… CARNET_S3_SECRET_KEY=…
   ```
2. **Back up.** `carnet-server backup ./carnet-backup-$(date +%F) --data ./data`, and copy `data/opaque.key` beside it (the backup leaves the keys out).
3. **The pictures first, while it runs.** On the server's machine (it reads the folder):
   ```bash
   carnet-server migrate --pictures-first --from ./data --to-s3 https://s3.eu-central-1.amazonaws.com/carnet-pictures
   ```
   It takes no lock and writes nothing in the folder, copies no records, and needs no database. It puts each picture the bucket lacks, skips one there already, and says how many of each and their bytes. A picture still coming in is left for the run after the stop; one dropped meanwhile may be copied anyway, and that run takes it from the bucket again. Run it again to catch up just before the stop. It refuses a bucket a server has run on, as `migrate` does.
4. **Stop the server** (`docker stop carnet`, `systemctl stop carnet-server`). Devices keep what they write and send it once a server answers again.
5. **Migrate.** The records, the pictures since step 3 (the rest are there already: it says so), the marks; then what it checked:
   ```bash
   carnet-server migrate --from ./data --to-postgres "$CARNET_POSTGRES" --to-s3 https://s3.eu-central-1.amazonaws.com/carnet-pictures
   ```
   Cut short (a network, a full disk)? Run it again: it goes on.
6. **Check again**, any time before the start: `carnet-server migrate … --verify-only` compares, and copies nothing.
7. **Start on them**, with the same `--data` (for its keys) and `--origin` as before:
   ```bash
   carnet-server --data ./data --origin https://notes.example.com --postgres "$CARNET_POSTGRES" --s3 https://s3.eu-central-1.amazonaws.com/carnet-pictures
   ```
   Then check: `/v1/health` is 200, a device syncs, a picture opens in the browser (CORS), `carnet_storage_failures_total` stays at 0.
8. **Going back.** Stop the new server and start the old command on the folder: it has everything as of step 4. What was written since is only in Postgres and the bucket: notes come back from the devices that hold them, as after any outage, but accounts, passwords and pictures added since don't. So go back soon, or not at all. To migrate again later, empty the database and the bucket's folder first: `migrate` won't copy over a server's work.

## What it serves

| | |
|---|---|
| `GET /` | what this is |
| `GET /v1/info` | `{"server", "version", "origin", "signup", "passwords", "password_email", "protocol", "oldest", "deprecated", "features"}`: which apps it takes and what it can do (docs/SYNC.md §14); not how many use it (that's `carnet_accounts` in `/metrics`) |
| `GET /v1/health` | `{"ok": true}` while the hub answers; `503` if not |
| `GET /v1/sync` | the WebSocket: Carnet's frames (log in, use an invite, pair, what changed) and the Loro Protocol's rooms, one per note |
| `GET /v1/blobs/<link>` | a sealed picture, by a link the socket gave: one picture, for minutes |
| `PUT /v1/blobs/<link>` | one put the same way, its `Content-Length` said: the length the device offered the picture as, at most 32 MiB |
| `GET /metrics` | on the metrics address only: Prometheus' text format |

## Tests

```bash
cargo test -p carnet-server
```

With Postgres and a bucket to test against, the storage tests (and the simulation, every third seed) run on Postgres too, each in a schema of its own, and the bucket's and the migration's tests run:

```bash
docker run -d --name carnet-pg -e POSTGRES_PASSWORD=carnet -p 127.0.0.1:5432:5432 postgres:17
docker run -d --name carnet-s3 -e RUSTFS_ACCESS_KEY=carnet -e RUSTFS_SECRET_KEY=carnetsecret -p 127.0.0.1:9000:9000 rustfs/rustfs
CARNET_TEST_POSTGRES='postgres://postgres:carnet@127.0.0.1/postgres?sslmode=disable' \
CARNET_TEST_S3=http://127.0.0.1:9000/carnet-test CARNET_S3_ACCESS_KEY=carnet CARNET_S3_SECRET_KEY=carnetsecret \
cargo test -p carnet-server --features postgres,s3
```

- `tests/sim.rs` simulates devices, the server and a faulty network from a seed, on memory and on SQLite alike (and Postgres, given one).
- `tests/migrate.rs` moves a folder to Postgres and a bucket and compares every answer; refuses a folder in use and a database served; copies the pictures first beside a running server, a picture half put meanwhile, then the rest after the stop; and end to end, fills a server on SQLite through its sockets, moves it, starts it on Postgres and the bucket, and its devices carry on (notes, a picture by the bucket's link, a password, a note deleted for good). `tests/s3.rs` puts and fetches by the bucket's links, marks and sweeps.
- `tests/server.rs` runs the binary's own path end to end: HTTP, WebSockets, SQLite, a restart, what health and metrics say, and the limits behind a trusted proxy; the data folder's permissions, a deleted account's links and pictures, and sockets from other sites' pages.
- `tests/auth.rs` sends what an attacker would, frame by frame: everything but a login before one, logins signed with the wrong key, for another nonce or server or account, false invites and recoveries, another account's rooms, devices and pictures, writes and snapshots out of turn, the owner's frames from someone else, and too many tries from one address.
