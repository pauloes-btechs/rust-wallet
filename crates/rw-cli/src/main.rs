//! `rw`: the rust-wallet command line.
//!
//! ```sh
//! rw --replay status                       # recorded litd regtest session, no node needed
//! rw --lnd-dir ~/.lnd --endpoint 127.0.0.1:10009 balance
//! rw pay lnbcrt1… --max-fee 20             # through the payment-intent engine
//! rw intents                               # what the engine has persisted
//! ```

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use rw_core::views::*;
use rw_core::{MainnetPolicy, Network, NodeApi, Sats};
use rw_litd::{ConnectConfig, LitdBackend};
use rw_payments::{IntentStore, RecoveryOutcome, SqliteStore};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "rw", version, about = "rust-wallet: a Lightning wallet over Lightning Labs' litd, in Rust")]
struct Cli {
    /// Answer from the recorded litd regtest session instead of a live node.
    #[arg(long, global = true)]
    replay: bool,
    /// LND/litd gRPC endpoint (host:port).
    #[arg(long, env = "RW_ENDPOINT", default_value = "127.0.0.1:10009", global = true)]
    endpoint: String,
    /// LND data dir; tls.cert and admin.macaroon are found inside it.
    #[arg(long, env = "RW_LND_DIR", global = true)]
    lnd_dir: Option<PathBuf>,
    #[arg(long, env = "RW_TLS_CERT", global = true)]
    tls_cert: Option<PathBuf>,
    #[arg(long, env = "RW_MACAROON", global = true)]
    macaroon: Option<PathBuf>,
    /// Network of the node (selects the macaroon path; the node's own report is enforced).
    #[arg(long, env = "RW_NETWORK", default_value = "regtest", global = true)]
    network: Network,
    /// Allow a mainnet node. Off until product sign-off.
    #[arg(long, global = true)]
    allow_mainnet: bool,
    /// Payment-intent database.
    #[arg(long, env = "RW_DB", global = true)]
    db: Option<PathBuf>,
    /// Print the raw view JSON.
    #[arg(long, global = true)]
    json: bool,
    /// Skip confirmation prompts for sends.
    #[arg(long, short = 'y', global = true)]
    yes: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Node identity, sync and wallet phase.
    Status,
    /// On-chain and channel balances.
    Balance,
    /// Open and pending channels.
    Channels,
    /// Open a channel: rw open <pubkey[@host:port]> <sats>.
    Open {
        peer: String,
        sats: u64,
        #[arg(long)]
        sat_per_vbyte: Option<u64>,
        #[arg(long)]
        private: bool,
    },
    Peers,
    /// New taproot receive address.
    Address,
    /// Fee estimate for an on-chain send.
    Estimate {
        address: String,
        sats: u64,
    },
    /// On-chain send (tracked by the payment engine via a unique tx label).
    Send {
        address: String,
        sats: u64,
        #[arg(long)]
        sat_per_vbyte: Option<u64>,
    },
    /// Create a lightning invoice.
    Invoice {
        sats: u64,
        #[arg(long, default_value = "")]
        memo: String,
    },
    Decode {
        invoice: String,
    },
    /// Pay a lightning invoice through the payment-intent engine.
    Pay {
        invoice: String,
        /// Only for zero-amount invoices.
        #[arg(long)]
        amount: Option<u64>,
        /// Routing-fee cap in sats (default: 1% of amount, min 10).
        #[arg(long)]
        max_fee: Option<u64>,
    },
    Payments,
    Invoices,
    /// On-chain transaction history.
    Txs,
    /// Export the static channel backup (multi-channel SCB).
    Backup {
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// litd sub-server status.
    Services,
    /// Taproot Assets balances.
    Assets,
    /// Loop swaps.
    Swaps,
    Diagnostics,
    /// Declared capabilities of the backend.
    Caps,
    /// Persisted payment intents.
    Intents,
    /// Reconcile interrupted payments (runs automatically on connect).
    Recover,
    /// Resolve a needs-recovery intent once its fate is known.
    Resolve {
        intent_id: String,
        /// Mark settled with this payment hash / txid.
        #[arg(long, conflicts_with = "failed")]
        settled: Option<String>,
        #[arg(long)]
        failed: bool,
    },
}

fn default_db() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| ".".into());
    home.join(".rust-wallet").join("intents.db")
}

fn open_store(cli: &Cli) -> Result<Arc<dyn IntentStore>> {
    let path = cli.db.clone().unwrap_or_else(default_db);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    Ok(Arc::new(SqliteStore::open(&path).with_context(|| format!("opening {}", path.display()))?))
}

fn connect_config(cli: &Cli) -> Result<ConnectConfig> {
    let mut cfg = match &cli.lnd_dir {
        Some(dir) => ConnectConfig::from_lnd_dir(cli.endpoint.clone(), dir, cli.network),
        None => {
            let (Some(c), Some(m)) = (&cli.tls_cert, &cli.macaroon) else {
                bail!("give --lnd-dir, or both --tls-cert and --macaroon (or use --replay)");
            };
            ConnectConfig {
                endpoint: cli.endpoint.clone(),
                tls_cert: c.clone(),
                macaroon: m.clone(),
                mainnet: MainnetPolicy::Refuse,
            }
        }
    };
    if let Some(c) = &cli.tls_cert {
        cfg.tls_cert = c.clone();
    }
    if let Some(m) = &cli.macaroon {
        cfg.macaroon = m.clone();
    }
    cfg.mainnet = if cli.allow_mainnet { MainnetPolicy::Allow } else { MainnetPolicy::Refuse };
    Ok(cfg)
}

fn confirm(cli: &Cli, what: &str) -> Result<()> {
    if cli.yes {
        return Ok(());
    }
    eprint!("{what} [y/N] ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    if matches!(line.trim(), "y" | "Y" | "yes") {
        Ok(())
    } else {
        bail!("cancelled")
    }
}

fn print<T: serde::Serialize>(cli: &Cli, v: &T, human: impl FnOnce(&T)) -> Result<()> {
    if cli.json {
        println!("{}", serde_json::to_string_pretty(v)?);
    } else {
        human(v);
    }
    Ok(())
}

fn short(s: &str) -> String {
    if s.len() > 20 {
        format!("{}…{}", &s[..10], &s[s.len() - 6..])
    } else {
        s.to_string()
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(&cli).await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run(cli: &Cli) -> Result<()> {
    if cli.replay {
        eprintln!(
            "· replay: litd {} on {}, recorded {}",
            rw_replay::meta().litd,
            rw_replay::meta().network,
            rw_replay::meta().recorded_at
        );
        if matches!(cli.cmd, Cmd::Intents | Cmd::Recover | Cmd::Resolve { .. }) {
            bail!("payment intents need a live node");
        }
        return commands(cli, &rw_replay::ReplayBackend::default()).await;
    }
    let cfg = connect_config(cli)?;
    let store = open_store(cli)?;
    let (backend, report) = LitdBackend::open(&cfg, store).await.context("connecting to the node")?;
    if !report.reconciled.is_empty() {
        eprintln!("· reconciled {} interrupted payment(s)", report.reconciled.len());
    }
    if !report.needs_recovery.is_empty() {
        eprintln!("! {} payment(s) need attention: run `rw intents`", report.needs_recovery.len());
    }
    match &cli.cmd {
        Cmd::Intents => {
            let v = backend.engine().list()?;
            return print(cli, &v, |v| {
                if v.is_empty() {
                    println!("no payment intents");
                }
                for i in v {
                    println!(
                        "{}  {:<14} {:<17} {:>12}  {}",
                        i.intent_id,
                        format!("{:?}", i.state),
                        format!("{:?}", i.kind),
                        i.amount_sat.map(|a| Sats(a).grouped()).unwrap_or_else(|| "invoice".into()),
                        i.backend_ref.as_deref().map(short).unwrap_or_default()
                    );
                }
            });
        }
        Cmd::Recover => {
            let r = backend.engine().recover_interrupted().await?;
            println!("reconciled: {:?}\nneeds recovery: {:?}", r.reconciled, r.needs_recovery);
            return Ok(());
        }
        Cmd::Resolve { intent_id, settled, failed } => {
            let outcome = match (settled, failed) {
                (Some(r), false) => RecoveryOutcome::Settled { backend_ref: r.clone() },
                (None, true) => RecoveryOutcome::Failed { reason: Some("resolved manually".into()) },
                _ => bail!("pass --settled <hash|txid> or --failed"),
            };
            let i = backend.engine().resolve_needs_recovery(intent_id, &outcome)?;
            println!("{} → {:?}", i.intent_id, i.state);
            return Ok(());
        }
        _ => {}
    }
    commands(cli, &backend).await
}

async fn commands<B: NodeApi>(cli: &Cli, b: &B) -> Result<()> {
    match &cli.cmd {
        Cmd::Status => print(cli, &b.state().await?, |s| {
            println!("phase      {:?}", s.phase);
            if let Some(i) = &s.info {
                println!("alias      {}", i.alias);
                println!("pubkey     {}", i.identity_pubkey);
                println!("version    {}", i.version);
                println!("network    {}", i.chains.first().map(|c| c.network.as_str()).unwrap_or("?"));
                println!(
                    "height     {}  (chain synced: {}, graph synced: {})",
                    i.block_height, i.synced_to_chain, i.synced_to_graph
                );
                println!(
                    "channels   {} active, {} pending, {} inactive · {} peers",
                    i.num_active_channels, i.num_pending_channels, i.num_inactive_channels, i.num_peers
                );
            }
            if let Some(e) = &s.error {
                println!("error      {e}");
            }
        }),
        Cmd::Balance => {
            let w = b.wallet_balance().await?;
            let c = b.channels().await?.balance;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "onchain": w, "channels": c }))?);
                return Ok(());
            }
            println!(
                "on-chain   {:>16}  ({} confirmed, {} unconfirmed)",
                Sats(w.total_sat).grouped(),
                Sats(w.confirmed_sat).grouped(),
                Sats(w.unconfirmed_sat).grouped()
            );
            println!("lightning  {:>16}  can send", Sats(c.local_sat).grouped());
            println!("           {:>16}  can receive", Sats(c.remote_sat).grouped());
            let total = w.total_sat.saturating_add(c.local_sat);
            println!("total      {:>16}  ({} BTC)", Sats(total).grouped(), Sats(total).btc_string());
            Ok(())
        }
        Cmd::Channels => print(cli, &b.channels().await?, |v| {
            for c in v.open.iter().chain(&v.pending) {
                let cap = c.capacity_sat.max(1) as f64;
                let bar = "█".repeat(((c.local_balance_sat as f64 / cap) * 20.0).round() as usize);
                println!(
                    "{:<14} {:>14} {:<20} {} {}",
                    format!("{:?}", c.state),
                    Sats(c.capacity_sat).grouped(),
                    bar,
                    short(&c.remote_pubkey),
                    c.initiator_label
                );
            }
            println!("local {} · remote {}", Sats(v.balance.local_sat), Sats(v.balance.remote_sat));
        }),
        Cmd::Open { peer, sats, sat_per_vbyte, private } => {
            confirm(cli, &format!("Open a {} channel to {}?", Sats(*sats), short(peer)))?;
            let r = b
                .open_channel(&OpenChannelRequest {
                    peer: peer.clone(),
                    local_funding_sat: *sats,
                    sat_per_vbyte: *sat_per_vbyte,
                    private: *private,
                })
                .await?;
            print(cli, &r, |r| println!("funding {}", r.channel_point))
        }
        Cmd::Peers => print(cli, &b.peers().await?, |v| {
            for p in &v.peers {
                println!(
                    "{} {:<22} {:>4}ms sent {} recv {}",
                    short(&p.pubkey),
                    p.address,
                    p.ping_ms,
                    Sats(p.sat_sent),
                    Sats(p.sat_received)
                );
            }
        }),
        Cmd::Address => print(cli, &b.new_address().await?, |a| println!("{}", a.address)),
        Cmd::Estimate { address, sats } => {
            let r = b
                .estimate_fee(&SendOnchainRequest { address: address.clone(), amount_sat: *sats, sat_per_vbyte: None })
                .await?;
            print(cli, &r, |r| println!("{} at {} sat/vB", Sats(r.fee_sat), r.sat_per_vbyte))
        }
        Cmd::Send { address, sats, sat_per_vbyte } => {
            let req = SendOnchainRequest { address: address.clone(), amount_sat: *sats, sat_per_vbyte: *sat_per_vbyte };
            let est = b.estimate_fee(&req).await?;
            confirm(cli, &format!("Send {} to {} (fee ≈ {})?", Sats(*sats), short(address), Sats(est.fee_sat)))?;
            print(cli, &b.send_onchain(&req).await?, |t| println!("broadcast {}", t.txid))
        }
        Cmd::Invoice { sats, memo } => {
            let r = b
                .create_invoice(&CreateInvoiceRequest { amount_sat: *sats, memo: memo.clone(), expiry_secs: None })
                .await?;
            print(cli, &r, |r| println!("{}", r.payment_request))
        }
        Cmd::Decode { invoice } => print(cli, &b.decode_invoice(invoice).await?, |d| {
            println!("amount     {}", d.amount_sat.map(|a| Sats(a).to_string()).unwrap_or_else(|| "any".into()));
            println!("memo       {}", d.memo);
            println!("payee      {}", d.destination);
            println!("hash       {}", d.payment_hash_hex);
        }),
        Cmd::Pay { invoice, amount, max_fee } => {
            let d = b.decode_invoice(invoice).await?;
            let amt = d.amount_sat.or(*amount).unwrap_or(0);
            let cap = max_fee.unwrap_or_else(|| rw_litd::default_fee_cap_sat(amt));
            confirm(
                cli,
                &format!("Pay {} to {} (\"{}\"), fee cap {}?", Sats(amt), short(&d.destination), d.memo, Sats(cap)),
            )?;
            let r = b
                .pay_invoice(&PayRequest {
                    payment_request: invoice.clone(),
                    amount_sat: *amount,
                    max_fee_sat: Some(cap),
                })
                .await?;
            print(cli, &r, |r| {
                println!("{:?}  hash {}", r.status, r.payment_hash);
                if let Some(f) = &r.failure_reason {
                    println!("reason     {f}");
                }
                if let Some(id) = &r.intent_id {
                    println!("intent     {id}");
                }
            })
        }
        Cmd::Payments => print(cli, &b.payments().await?, |v| {
            for p in &v.payments {
                println!(
                    "{:<10} {:>14} fee {:>6}  {}",
                    p.status,
                    Sats(p.value_sat).grouped(),
                    p.fee_sat,
                    short(&p.payment_hash)
                );
            }
        }),
        Cmd::Invoices => print(cli, &b.invoices().await?, |v| {
            for i in &v.invoices {
                println!(
                    "{:<9} {:>14}  {:<24} {}",
                    i.state,
                    Sats(i.value_sat).grouped(),
                    i.memo,
                    short(&i.payment_hash_hex)
                );
            }
        }),
        Cmd::Txs => print(cli, &b.transactions().await?, |v| {
            for t in &v.transactions {
                println!("{:>+16} {:>4} conf  {}  {}", t.amount_sat, t.confirmations, short(&t.txid), t.label);
            }
        }),
        Cmd::Backup { out } => {
            let r = b.channel_backup().await?;
            if let Some(path) = out {
                std::fs::write(path, &r.multi_chan_backup_hex)?;
                println!("wrote {} bytes of SCB hex to {}", r.multi_chan_backup_hex.len(), path.display());
                Ok(())
            } else {
                print(cli, &r, |r| println!("{}", r.multi_chan_backup_hex))
            }
        }
        Cmd::Services => print(cli, &b.services().await?, |v| {
            println!("network {} · ready {}", v.network, v.ready);
            for (k, s) in &v.sub_servers {
                println!(
                    "  {:<16} {}{}",
                    k,
                    if s.running {
                        "running"
                    } else if s.disabled {
                        "disabled"
                    } else {
                        "stopped"
                    },
                    if s.error.is_empty() { String::new() } else { format!("  ({})", s.error) }
                );
            }
        }),
        Cmd::Assets => print(cli, &b.assets().await?, |v| {
            for a in &v.balances {
                println!("{:<24} {:>14}  {} {}", a.name, a.balance, a.asset_type, short(&a.asset_id_hex));
            }
            if v.balances.is_empty() {
                println!("no taproot assets");
            }
        }),
        Cmd::Swaps => print(cli, &b.swaps().await?, |v| {
            for s in &v.swaps {
                println!("{:<9} {:<12} {:>12} cost {}", s.swap_type, s.state, Sats(s.amount_sat).grouped(), s.cost_sat);
            }
            if v.swaps.is_empty() {
                println!("no swaps");
            }
        }),
        Cmd::Diagnostics => print(cli, &b.diagnostics().await?, |d| {
            println!(
                "{} · height {} · {} closed · {} forwards",
                d.node.alias,
                d.node.block_height,
                d.closed.len(),
                d.forwarding.len()
            );
            println!(
                "faraday: {}",
                if d.faraday.available {
                    format!("{} channel insights", d.faraday.insights.len())
                } else {
                    d.faraday.error.clone().unwrap_or_default()
                }
            );
        }),
        Cmd::Caps => print(cli, b.capabilities(), |c| {
            for c in c.iter() {
                println!("{c:?}");
            }
        }),
        Cmd::Intents | Cmd::Recover | Cmd::Resolve { .. } => unreachable!("handled before"),
    }
}
