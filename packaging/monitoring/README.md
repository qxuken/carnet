# Watching carnet-server: Grafana, Prometheus, Loki

A dashboard, alerts and the two small services that feed them, for a
carnet-server in Docker. Written for a home server (Unraid) that has
Grafana and Loki already, but not Prometheus; anywhere else it's the same
with other paths.

| File | What it is |
|---|---|
| [`carnet-server.json`](carnet-server.json) | The Grafana dashboard: health, traffic, data, and the server's lines |
| [`alerts.yaml`](alerts.yaml) | Grafana alert rules, and a webhook contact point that hands them to n8n |
| [`datasources.yaml`](datasources.yaml) | Grafana's Prometheus data source, with the UID the alerts name |
| [`prometheus.yml`](prometheus.yml) | Prometheus, reading the server every 15 seconds |
| [`config.alloy`](config.alloy) | Grafana Alloy, taking the server container's lines to Loki |
| [`compose.yaml`](compose.yaml) | Prometheus and Alloy as containers, for Docker Compose |

The server's numbers are counts only: how many connections, logins,
records, bytes. Never what a note says, nor whose it is.

## How it fits together

```
carnet-server (br0, 192.168.50.53)
   :8080  ← Cloudflare Tunnel ← the internet          (devices)
   :9090  /metrics  ← Prometheus (br0, .54) ← Grafana  (numbers, every 15 s)
   its lines → Docker → Alloy → Loki ← Grafana         (logs)
                                Grafana alerts → n8n   (a webhook)
```

**Prometheus reads the server itself; Alloy carries only the lines.**
Alloy can read `/metrics` too and write it on to Prometheus (the commented
part of `config.alloy`), but then Prometheus has to take writes and there
are two places a scrape can go wrong. Prometheus reading the server
directly is one hop, and its Targets page says plainly when it can't.
Loki is different: it only takes what it's sent, so something has to send
it the lines. That's Alloy (Promtail did it before; it's deprecated).

The addresses below are examples: the server at `192.168.50.53`,
Prometheus at `192.168.50.54`, Unraid itself at `192.168.50.10`, n8n at
`192.168.50.20:5678`. Put in yours.

## 1. carnet-server: turn the numbers on

In the server container's template (Docker tab → carnet → Edit), add a
Variable:

| Name | Key | Value |
|---|---|---|
| Metrics | `CARNET_METRICS` | `0.0.0.0:9090` |

Apply. `docker logs carnet` now says `metrics on http://0.0.0.0:9090/metrics`.
On `br0` the container has an address of its own, so there's no port to
map: `http://192.168.50.53:9090/metrics` answers as soon as it's on.

**Who can see it.** On `br0` every port the server listens on is open to
your whole LAN, the metrics one too. That's counts only, no note, no
name, no key; still, keep it on the LAN. Cloudflare's tunnel maps your
public hostname to `http://192.168.50.53:8080` and nothing else: don't add
a hostname (or a path) for `:9090`. The public address doesn't serve
`/metrics` at all; only the metrics listener does.

Check, from any machine on the LAN:

```bash
curl -s http://192.168.50.53:9090/metrics | head
curl -s http://192.168.50.53:9090/v1/health      # {"ok":true}
```

## 2. Prometheus

**Which network.** Containers on `br0` reach each other. A container on a
bridge network (Unraid's default `bridge`, or a Compose network) can't
reach a `br0` address, nor can Unraid itself: that's how macvlan works.
Two ways round it; pick one:

- Put Prometheus on `br0` too, with an IP of its own (what `compose.yaml`
  does). It reaches the server; Grafana reaches it if Grafana is on `br0`
  as well, or with the setting below.
- Or turn on **Settings → Docker → Host access to custom networks**
  (Advanced View; stop Docker to change it). Then bridge containers reach
  `br0` ones, and Prometheus can sit on `bridge` with its port mapped.

**Files.**

```bash
mkdir -p /mnt/user/appdata/prometheus/data
cp prometheus.yml /mnt/user/appdata/prometheus/
```

Edit `/mnt/user/appdata/prometheus/prometheus.yml`: the target is the
server's IP and metrics port, `192.168.50.53:9090`. Keep `job_name:
carnet`; the dashboard and the alerts look for it.

**The container.** With the Compose Manager plugin, `compose.yaml` here
(edit Prometheus' IP first). Or Docker tab → Add Container:

| Field | Value |
|---|---|
| Name | `prometheus` |
| Repository | `prom/prometheus:v3.15.0` |
| Network Type | `Custom: br0`, Fixed IP `192.168.50.54` |
| Extra Parameters | `--user 99:100` |
| Post Arguments | `--config.file=/etc/prometheus/prometheus.yml --storage.tsdb.path=/prometheus --storage.tsdb.retention.time=30d` |
| Path | container `/etc/prometheus/prometheus.yml` → host `/mnt/user/appdata/prometheus/prometheus.yml` (read only) |
| Path | container `/prometheus` → host `/mnt/user/appdata/prometheus/data` |

`--user 99:100` is Unraid's `nobody:users`, who owns `appdata`;
Prometheus' own user can't write there and stops with "permission
denied". `retention.time=30d` keeps a month; a home server's month is a
few hundred megabytes at most.

Check: `http://192.168.50.54:9090/targets` lists `carnet` as **UP**.

## 3. Alloy: the server's lines to Loki

Alloy asks Docker for the container's lines through Docker's socket, so
it doesn't matter which network the server is on. It must reach Loki.

```bash
mkdir -p /mnt/user/appdata/alloy/data
cp config.alloy /mnt/user/appdata/alloy/
```

Edit `/mnt/user/appdata/alloy/config.alloy`:

- `url` in `loki.write`: Loki's push address as Alloy's container reaches
  it. Loki on `bridge` with its port mapped: `http://192.168.50.10:3100/loki/api/v1/push`
  (Unraid's IP). Loki and Alloy on the same Docker network:
  `http://loki:3100/loki/api/v1/push`. Loki on `br0`: its IP, with the
  network note above.
- `regex = "/carnet"`: the server container's name, after a slash. Unraid
  names containers after the template (`carnet-server`?): check
  `docker ps`. To send every container's lines, delete that `keep` rule.

The container (or `compose.yaml`):

| Field | Value |
|---|---|
| Name | `alloy` |
| Repository | `grafana/alloy:v1.20.1` |
| Network Type | `bridge` (or Loki's network) |
| Post Arguments | `run --server.http.listen-addr=0.0.0.0:12345 --storage.path=/var/lib/alloy/data /etc/alloy/config.alloy` |
| Path | container `/var/run/docker.sock` → host `/var/run/docker.sock` (read only) |
| Path | container `/etc/alloy/config.alloy` → host `/mnt/user/appdata/alloy/config.alloy` (read only) |
| Path | container `/var/lib/alloy/data` → host `/mnt/user/appdata/alloy/data` |
| Port | `12345` → `12345` (its own page) |

The lines come with labels `container` (`carnet`), `compose_service` when
Compose started it, and `service_name`, which Loki adds. Docker doesn't
tell Alloy a container's image, so there's no `image` label.

The first start of a server with no accounts prints the owner's invite
code. It's single use and good for a day, but it has no business in Loki:
`config.alloy` masks any such code to `XXXX-XXXX-XXXX-XXXX` on the way.
Read it with `docker logs carnet`, as before.

Check: `http://192.168.50.10:12345` shows every component green, and
Grafana's Explore, on Loki, finds `{container="carnet"}`.

## 4. Grafana

### Prometheus as a data source

Either way works; the first saves a step with the alerts.

- **Provisioned.** Copy `datasources.yaml` into Grafana's
  `provisioning/datasources` folder (below), set its `url` to Prometheus
  as Grafana reaches it (`http://192.168.50.54:9090`), restart Grafana. Its
  UID is `prometheus`, which `alerts.yaml` names.
- **By hand.** Connections → Data sources → Add → Prometheus. URL
  `http://192.168.50.54:9090`; under Interval behaviour, Scrape interval
  `15s`. Save & test. Its UID is the last part of the page's address
  (`…/datasources/edit/<UID>`): put it in `alerts.yaml` for `prometheus`:

  ```bash
  sed -i 's/datasourceUid: prometheus/datasourceUid: <UID>/' alerts.yaml
  ```

Your Loki data source is fine as it is.

### The dashboard

Dashboards → New → Import → Upload `carnet-server.json`. It asks for the
Prometheus and the Loki to use; pick them, Import.

At the top: Job and Instance (one server, one of each), and Container,
the name Alloy labels the lines with (`carnet`, or yours; a regular
expression). Four rows:

- **Health**: up or down, version, how long it's been running, failed
  writes (red on any), how far behind its hub is, devices online; the
  hub's lag and the time it takes over each message (p50, p95, p99).
- **Traffic**: connections and devices logged in, by protocol and by app;
  logins let in and refused; connections turned away, and closed by the
  server's limits; messages and bytes each way; logins by app.
- **Data**: accounts, what the databases take on disk, records and
  pictures kept, batches refused, pairings; then those over time,
  pictures' links and transfers, pictures let go, and new devices
  (invites, pairings, recoveries).
- **Logs**: the server's lines that speak of trouble (empty is good), and
  all its lines.

Every panel says what it shows: hover its title. "In the time shown"
panels count over the time picked at the top right.

### The alerts

Grafana takes `alerts.yaml` from its `provisioning/alerting` folder when
it starts. First edit the n8n `url` in it (below).

| Rule | When | For |
|---|---|---|
| Carnet server is down | Prometheus can't read `/metrics`, or has nothing for it (`up` 0 or gone) | 2 min |
| Carnet server can't write | any failed write in 10 minutes (`carnet_storage_failures_total`) | at once |
| Carnet server is falling behind | the hub's lag over 1 s (`carnet_hub_queue_seconds`) | 5 min |
| Many Carnet logins refused | over 30 in 15 minutes | at once |
| Carnet data growing fast | the databases grew over 1 GiB in an hour (optional) | at once |

"Down" covers health too: when the server's hub stops answering,
`/metrics` answers 503, as `/v1/health` does, and the scrape fails. Disk
space itself is the machine's business: Unraid tells you, or
`node_exporter` does for Prometheus.

**Provisioning folders on Unraid.** Grafana's container needs the folders
mapped. Docker tab → Grafana → Edit → Add another Path, Port, Variable:

| Container path | Host path |
|---|---|
| `/etc/grafana/provisioning/alerting` | `/mnt/user/appdata/grafana/provisioning/alerting` |
| `/etc/grafana/provisioning/datasources` | `/mnt/user/appdata/grafana/provisioning/datasources` (if you provision Prometheus) |

```bash
mkdir -p /mnt/user/appdata/grafana/provisioning/{alerting,datasources}
cp alerts.yaml /mnt/user/appdata/grafana/provisioning/alerting/carnet.yaml
cp datasources.yaml /mnt/user/appdata/grafana/provisioning/datasources/carnet-prometheus.yaml
chmod 644 /mnt/user/appdata/grafana/provisioning/*/*.yaml
```

Restart Grafana. Alerting → Alert rules has a **Carnet** folder with the
five rules; Alerting → Contact points has `carnet-n8n`. Rules provisioned
from a file can't be edited in Grafana's pages: change the file, then
restart Grafana.

### n8n

Each rule sends to the contact point `carnet-n8n`, a webhook, by itself:
your notification policies stay as they are. To route by policy instead,
delete the `notification_settings` lines; the rules carry `service=carnet`.

1. In n8n, a new workflow with a **Webhook** node: HTTP Method `POST`, Path
   `carnet-alerts`. Its Production URL is
   `http://192.168.50.20:5678/webhook/carnet-alerts`; put it in
   `alerts.yaml` under `contactPoints` → `url`.
2. After it, what should tell you: Telegram, email, ntfy… Grafana's body
   has `status` (`firing` or `resolved`), `title` (like
   `[FIRING:1] Carnet server is down`), `message` (the summaries, ready to
   send), and `alerts`, each with its `labels` and `annotations`. A
   Telegram node's text could be `{{ $json.body.title }}` then
   `{{ $json.body.message }}`.
3. Activate the workflow. In Grafana, Alerting → Contact points →
   carnet-n8n → Test: n8n's Executions should show it.

Grafana must reach n8n: the same network rules as above.

## 5. Is it all working?

```bash
# the server
curl -s http://192.168.50.53:9090/metrics | head
# Prometheus has it
curl -s 'http://192.168.50.54:9090/api/v1/query?query=up{job="carnet"}'
```

In Grafana's Explore:

| Data source | Query | Expect |
|---|---|---|
| Prometheus | `up{job="carnet"}` | 1 |
| Prometheus | `carnet_build_info` | the version as a label |
| Prometheus | `carnet_sockets_logged_in` | your devices online |
| Prometheus | `rate(carnet_frames_received_total[5m])` | above 0 while a device syncs |
| Loki | `{container="carnet"}` | `carnet-server: listening on 0.0.0.0:8080, origin https://…` |

Then the dashboard: every panel with a line or a number. If one says
**No data**:

- *Everything*: Job at the top is empty, so Prometheus has nothing. Its
  Targets page says why; "context deadline exceeded" or "no route to host"
  is the network note in step 2.
- *Server*: the target isn't `job="carnet"`; keep that job name.
- *The two log panels*: the Container box doesn't match the `container`
  label, or Alloy can't reach Loki (its page, port 12345, says so in red),
  or can't read Docker's socket (mapped read only, as above).
- *Trouble*: empty is the good case.

## Tried with

carnet-server 0.1.1, Prometheus 3.15.0, Loki 3.7.8, Alloy 1.20.1 and
Grafana 13.2.4, all in Docker: the dashboard imported and provisioned,
every panel with data, the server's lines in Loki with the invite masked,
the alerts loaded, and "down" firing to a webhook 2 minutes after the
server stopped, then resolved when it came back.
