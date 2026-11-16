use crate::api;
use crate::components::*;
use crate::fmt::{btc, pct, sats, short, when_ms, when_ns};
use leptos::prelude::*;
use leptos::task::spawn_local;
use rw_core::destination::{parse_destination, Destination};
use rw_core::views::*;
use rw_core::Network;

/// Node-reported network, provided by the app shell.
#[derive(Clone, Copy)]
pub struct Net(pub ReadSignal<Option<Network>>);

fn net() -> Option<Network> {
    use_context::<Net>().and_then(|n| n.0.get())
}

// ------------------------------------------------------------------ overview

#[component]
pub fn Overview() -> impl IntoView {
    let wallet = load(api::wallet_balance);
    let chans = load(api::channels);
    let pays = load(api::payments);
    let invs = load(api::invoices);
    view! {
        <div class="grid">
            <Card title="Balance">
                {move || match (wallet.get(), chans.get()) {
                    (Some(Ok(w)), Some(Ok(c))) => {
                        let total = w.total_sat.saturating_add(c.balance.local_sat);
                        view! {
                            <div class="hero">
                                <div class="hero-amount">{sats(total)}<span class="unit">" sats"</span></div>
                                <div class="muted">{btc(total)}" BTC"</div>
                            </div>
                            <div class="stats">
                                <Stat label="On-chain" value=sats(w.total_sat) sub=format!("{} unconfirmed", sats(w.unconfirmed_sat)) />
                                <Stat label="Lightning · can send" value=sats(c.balance.local_sat) sub=format!("{} channels", c.open.len()) />
                                <Stat label="Lightning · can receive" value=sats(c.balance.remote_sat) sub="inbound liquidity" />
                            </div>
                        }.into_any()
                    }
                    (Some(Err(e)), _) | (_, Some(Err(e))) => view! { <ErrorBox msg=e.to_string() /> }.into_any(),
                    _ => view! { <div class="muted pad">"Loading…"</div> }.into_any(),
                }}
            </Card>
            <Card title="Channel liquidity">
                {panel(chans, |c: ChannelsView| {
                    let rows = c.open.into_iter().map(|ch| view! {
                        <div class="row">
                            <span class="mono">{short(&ch.remote_pubkey)}</span>
                            <Liquidity local=ch.local_balance_sat remote=ch.remote_balance_sat />
                            <span class="num">{sats(ch.capacity_sat)}</span>
                        </div>
                    }).collect::<Vec<_>>();
                    view! { <div class="rows">{rows}</div> }
                })}
            </Card>
            <Card title="Recent lightning activity">
                {move || match (pays.get(), invs.get()) {
                    (Some(Ok(p)), Some(Ok(i))) => {
                        let mut rows: Vec<(i64, String, String, String)> = p.payments.iter().map(|p| {
                            ((p.creation_time_ns / 1_000_000) as i64, "Sent".to_string(), format!("−{}", sats(p.value_sat)), p.status.clone())
                        }).collect();
                        rows.extend(i.invoices.iter().filter(|i| i.state == "settled").map(|i| {
                            (i.settled_at.unwrap_or(i.created_at), "Received".to_string(), format!("+{}", sats(i.paid_sat)), i.memo.clone())
                        }));
                        rows.sort_by_key(|r| std::cmp::Reverse(r.0));
                        let list = rows.into_iter().take(8).map(|(t, kind, amt, note)| view! {
                            <div class="row">
                                <span class="muted">{when_ms(t)}</span>
                                <span>{kind}</span>
                                <span class="muted ellipsis">{note}</span>
                                <span class="num">{amt}</span>
                            </div>
                        }).collect::<Vec<_>>();
                        view! { <div class="rows">{list}</div> }.into_any()
                    }
                    (Some(Err(e)), _) | (_, Some(Err(e))) => view! { <ErrorBox msg=e.to_string() /> }.into_any(),
                    _ => view! { <div class="muted pad">"Loading…"</div> }.into_any(),
                }}
            </Card>
        </div>
    }
}

// ---------------------------------------------------------------------- send

#[derive(Clone, PartialEq)]
enum Review {
    Invoice(DecodedInvoiceView),
    Onchain(FeeEstimateView),
}

#[component]
pub fn Send() -> impl IntoView {
    let (dest, set_dest) = signal(String::new());
    let (amount, set_amount) = signal(String::new());
    let (max_fee, set_max_fee) = signal(String::new());
    let (review, set_review) = signal(None::<Review>);
    let (busy, set_busy) = signal(false);
    let (result, set_result) = signal(None::<Result<String, String>>);

    let parsed = move || {
        let d = dest.get();
        if d.trim().is_empty() {
            None
        } else {
            Some(parse_destination(&d, net()))
        }
    };
    let amount_sat = move || amount.get().replace(',', "").trim().parse::<u64>().ok();

    let do_review = move |_| {
        set_result.set(None);
        let Some(Ok(d)) = parsed() else { return };
        set_busy.set(true);
        spawn_local(async move {
            let r = match &d {
                Destination::Bolt11 { invoice, .. } => api::decode_invoice(invoice).await.map(Review::Invoice),
                _ => match amount_sat() {
                    Some(a) if a > 0 => api::estimate_fee(&SendOnchainRequest {
                        address: d.as_str().to_string(),
                        amount_sat: a,
                        sat_per_vbyte: None,
                    })
                    .await
                    .map(Review::Onchain),
                    _ => Err(rw_core::ApiError::Invalid("enter an amount in sats".into())),
                },
            };
            match r {
                Ok(r) => set_review.set(Some(r)),
                Err(e) => set_result.set(Some(Err(e.to_string()))),
            }
            set_busy.set(false);
        });
    };

    let do_send = move |_| {
        let Some(rv) = review.get() else { return };
        set_busy.set(true);
        let d = dest.get();
        let fee = max_fee.get().trim().parse::<u64>().ok();
        let amt = amount_sat();
        spawn_local(async move {
            let r = match rv {
                Review::Invoice(inv) => api::pay_invoice(&PayRequest {
                    payment_request: d.trim().to_string(),
                    amount_sat: if inv.amount_sat.is_some() { None } else { amt },
                    max_fee_sat: fee,
                })
                .await
                .map(|o| match o.status {
                    PaymentOutcomeStatus::Succeeded => format!("Paid · hash {}", short(&o.payment_hash)),
                    PaymentOutcomeStatus::Failed => format!("Failed: {}", o.failure_reason.unwrap_or_default()),
                    PaymentOutcomeStatus::NoResult => {
                        "Sent, result pending. It will be reconciled, never re-sent. See Journal.".to_string()
                    }
                }),
                Review::Onchain(_) => api::send_onchain(&SendOnchainRequest {
                    address: d.trim().to_string(),
                    amount_sat: amt.unwrap_or(0),
                    sat_per_vbyte: None,
                })
                .await
                .map(|t| format!("Broadcast · txid {}", short(&t.txid))),
            };
            set_result.set(Some(r.map_err(|e| e.to_string())));
            set_review.set(None);
            set_busy.set(false);
            bump();
        });
    };

    view! {
        <div class="grid narrow">
            <Card title="Send">
                <label class="field">
                    <span>"Invoice or bitcoin address"</span>
                    <textarea
                        rows="3"
                        placeholder="lnbcrt… or bcrt1…"
                        spellcheck="false"
                        prop:value=move || dest.get()
                        on:input=move |ev| { set_dest.set(event_target_value(&ev)); set_review.set(None); }
                    ></textarea>
                </label>
                {move || parsed().map(|r| match r {
                    Ok(Destination::Bolt11 { network, amount_msat, .. }) => view! {
                        <div class="hint ok">
                            "⚡ Lightning invoice · " {network.to_string()}
                            {amount_msat.map(|m| format!(" · {} sats", sats(m / 1000))).unwrap_or_else(|| " · any amount".into())}
                        </div>
                    }.into_any(),
                    Ok(Destination::SegwitAddress { network, witness_version, .. }) => view! {
                        <div class="hint ok">"₿ " {if witness_version == 1 { "Taproot" } else { "SegWit" }} " address · " {network.to_string()}</div>
                    }.into_any(),
                    Ok(Destination::LegacyAddress { .. }) => view! { <div class="hint">"Legacy address: the node will verify it"</div> }.into_any(),
                    Err(e) => view! { <div class="hint bad">{e.to_string()}</div> }.into_any(),
                })}
                <div class="two">
                    <label class="field">
                        <span>"Amount (sats)"</span>
                        <input inputmode="numeric" placeholder="from invoice" prop:value=move || amount.get()
                            on:input=move |ev| set_amount.set(event_target_value(&ev)) />
                    </label>
                    <label class="field">
                        <span>"Max routing fee (sats)"</span>
                        <input inputmode="numeric" placeholder="1%, min 10" prop:value=move || max_fee.get()
                            on:input=move |ev| set_max_fee.set(event_target_value(&ev)) />
                    </label>
                </div>
                <button class="primary" disabled=move || busy.get() || !matches!(parsed(), Some(Ok(_))) on:click=do_review>
                    "Review"
                </button>
            </Card>
            {move || review.get().map(|rv| {
                let body = match rv {
                    Review::Invoice(d) => view! {
                        <dl class="kv">
                            <dt>"Amount"</dt><dd>{d.amount_sat.map(sats).unwrap_or_else(|| "you choose".into())}" sats"</dd>
                            <dt>"Memo"</dt><dd>{if d.memo.is_empty() { "—".to_string() } else { d.memo.clone() }}</dd>
                            <dt>"Payee"</dt><dd class="mono">{short(&d.destination)}</dd>
                            <dt>"Payment hash"</dt><dd class="mono">{short(&d.payment_hash_hex)}</dd>
                        </dl>
                    }.into_any(),
                    Review::Onchain(f) => view! {
                        <dl class="kv">
                            <dt>"Amount"</dt><dd>{amount_sat().map(sats).unwrap_or_default()}" sats"</dd>
                            <dt>"Network fee"</dt><dd>{sats(f.fee_sat)}" sats at "{f.sat_per_vbyte}" sat/vB"</dd>
                        </dl>
                    }.into_any(),
                };
                view! {
                    <Card title="Confirm">
                        {body}
                        <p class="muted small">
                            "The payment is recorded before it is sent. If the app stops mid-payment, it checks the node on restart and never sends twice."
                        </p>
                        <button class="primary" disabled=move || busy.get() on:click=do_send>"Send now"</button>
                    </Card>
                }
            })}
            {move || result.get().map(|r| match r {
                Ok(m) => view! { <div class="success">{m}</div> }.into_any(),
                Err(e) => view! { <ErrorBox msg=e /> }.into_any(),
            })}
        </div>
    }
}

// ------------------------------------------------------------------- receive

#[component]
pub fn Receive() -> impl IntoView {
    let (lightning, set_lightning) = signal(true);
    let (amount, set_amount) = signal(String::new());
    let (memo, set_memo) = signal(String::new());
    let (out, set_out) = signal(None::<Result<String, String>>);
    let (busy, set_busy) = signal(false);

    let create = move |_| {
        set_busy.set(true);
        let is_ln = lightning.get();
        let amt = amount.get().replace(',', "").trim().parse::<u64>().unwrap_or(0);
        let m = memo.get();
        spawn_local(async move {
            let r = if is_ln {
                api::create_invoice(&CreateInvoiceRequest { amount_sat: amt, memo: m, expiry_secs: None })
                    .await
                    .map(|i| i.payment_request)
            } else {
                api::new_address().await.map(|a| a.address)
            };
            set_out.set(Some(r.map_err(|e| e.to_string())));
            set_busy.set(false);
        });
    };

    view! {
        <div class="grid narrow">
            <Card title="Receive">
                <div class="seg">
                    <button class:on=move || lightning.get() on:click=move |_| { set_lightning.set(true); set_out.set(None); }>"Lightning"</button>
                    <button class:on=move || !lightning.get() on:click=move |_| { set_lightning.set(false); set_out.set(None); }>"On-chain"</button>
                </div>
                <Show when=move || lightning.get()>
                    <div class="two">
                        <label class="field"><span>"Amount (sats)"</span>
                            <input inputmode="numeric" prop:value=move || amount.get() on:input=move |ev| set_amount.set(event_target_value(&ev)) /></label>
                        <label class="field"><span>"Memo"</span>
                            <input prop:value=move || memo.get() on:input=move |ev| set_memo.set(event_target_value(&ev)) /></label>
                    </div>
                </Show>
                <button class="primary" disabled=move || busy.get() on:click=create>
                    {move || if lightning.get() { "Create invoice" } else { "New taproot address" }}
                </button>
            </Card>
            {move || out.get().map(|r| match r {
                Ok(s) => view! {
                    <Card title=if lightning.get() { "Invoice" } else { "Address" }>
                        <Qr data=s.clone() />
                        <div class="mono wrap">{s.clone()}</div>
                        <Copy text=s />
                    </Card>
                }.into_any(),
                Err(e) => view! { <ErrorBox msg=e /> }.into_any(),
            })}
        </div>
    }
}

// ------------------------------------------------------------------ channels

fn state_label(s: ChannelState) -> &'static str {
    match s {
        ChannelState::Opening => "opening",
        ChannelState::Open => "open",
        ChannelState::OpenInactive => "inactive",
        ChannelState::Closing => "closing",
        ChannelState::ClosingForce => "force-closing",
        ChannelState::WaitingClose => "waiting close",
    }
}

#[component]
pub fn Channels() -> impl IntoView {
    let chans = load(api::channels);
    let (peer, set_peer) = signal(String::new());
    let (amt, set_amt) = signal(String::new());
    let (msg, set_msg) = signal(None::<Result<String, String>>);
    let open = move |_| {
        let req = OpenChannelRequest {
            peer: peer.get().trim().to_string(),
            local_funding_sat: amt.get().replace(',', "").trim().parse().unwrap_or(0),
            sat_per_vbyte: None,
            private: false,
        };
        spawn_local(async move {
            let r = api::open_channel(&req).await.map(|r| format!("Funding broadcast: {}", short(&r.channel_point)));
            set_msg.set(Some(r.map_err(|e| e.to_string())));
            bump();
        });
    };
    view! {
        <div class="grid">
            <Card title="Channels">
                {panel(chans, |c: ChannelsView| {
                    let rows = c.open.into_iter().chain(c.pending).map(|ch| view! {
                        <div class="chan">
                            <div class="chan-top">
                                <span class="mono">{short(&ch.remote_pubkey)}</span>
                                <span class=format!("chip {}", state_label(ch.state).replace(' ', "-"))>{state_label(ch.state)}</span>
                            </div>
                            <Liquidity local=ch.local_balance_sat remote=ch.remote_balance_sat />
                            <div class="chan-bottom muted small">
                                <span>{format!("{} local", sats(ch.local_balance_sat))}</span>
                                <span>{format!("{} capacity · {}", sats(ch.capacity_sat), ch.commitment_type.clone().unwrap_or_else(|| ch.initiator_label.clone()))}</span>
                                <span>{format!("{} remote", sats(ch.remote_balance_sat))}</span>
                            </div>
                        </div>
                    }).collect::<Vec<_>>();
                    let local = c.balance.local_sat;
                    let remote = c.balance.remote_sat;
                    view! {
                        <div class="muted small pad-b">{format!("Outbound {:.0}% · inbound {:.0}%", pct(local, local + remote), pct(remote, local + remote))}</div>
                        <div class="chans">{rows}</div>
                    }
                })}
            </Card>
            <Card title="Open a channel">
                <label class="field"><span>"Peer (pubkey@host:port)"</span>
                    <input class="mono" spellcheck="false" prop:value=move || peer.get() on:input=move |ev| set_peer.set(event_target_value(&ev)) /></label>
                <label class="field"><span>"Funding (sats)"</span>
                    <input inputmode="numeric" prop:value=move || amt.get() on:input=move |ev| set_amt.set(event_target_value(&ev)) /></label>
                <button class="primary" on:click=open>"Open channel"</button>
                {move || msg.get().map(|r| match r {
                    Ok(m) => view! { <div class="success">{m}</div> }.into_any(),
                    Err(e) => view! { <ErrorBox msg=e /> }.into_any(),
                })}
            </Card>
        </div>
    }
}

// ------------------------------------------------------------------ activity

#[component]
pub fn Activity() -> impl IntoView {
    let pays = load(api::payments);
    let invs = load(api::invoices);
    let txs = load(api::transactions);
    view! {
        <div class="grid">
            <Card title="Payments sent">
                {panel(pays, |p: PaymentsView| view! {
                    <table><thead><tr><th>"When"</th><th>"Status"</th><th class="num">"Amount"</th><th class="num">"Fee"</th><th>"Hash"</th></tr></thead>
                    <tbody>{p.payments.into_iter().map(|p| view! {
                        <tr><td>{when_ns(p.creation_time_ns)}</td><td><span class=format!("chip {}", p.status)>{p.status.clone()}</span></td>
                        <td class="num">{sats(p.value_sat)}</td><td class="num">{sats(p.fee_sat)}</td><td class="mono">{short(&p.payment_hash)}</td></tr>
                    }).collect::<Vec<_>>()}</tbody></table>
                })}
            </Card>
            <Card title="Invoices">
                {panel(invs, |i: InvoicesView| view! {
                    <table><thead><tr><th>"Created"</th><th>"State"</th><th class="num">"Amount"</th><th>"Memo"</th></tr></thead>
                    <tbody>{i.invoices.into_iter().map(|i| view! {
                        <tr><td>{when_ms(i.created_at)}</td><td><span class=format!("chip {}", i.state)>{i.state.clone()}</span></td>
                        <td class="num">{sats(i.value_sat)}</td><td class="ellipsis">{i.memo.clone()}</td></tr>
                    }).collect::<Vec<_>>()}</tbody></table>
                })}
            </Card>
            <Card title="On-chain">
                {panel(txs, |t: TransactionsView| view! {
                    <table><thead><tr><th>"When"</th><th class="num">"Amount"</th><th class="num">"Conf"</th><th>"Label"</th><th>"Txid"</th></tr></thead>
                    <tbody>{t.transactions.into_iter().map(|t| {
                        let cls = if t.amount_sat < 0 { "num out" } else { "num in" };
                        view! {
                            <tr><td>{when_ms(t.at)}</td><td class=cls>{format!("{:+}", t.amount_sat)}</td>
                            <td class="num">{t.confirmations}</td><td class="ellipsis muted">{t.label.clone()}</td><td class="mono">{short(&t.txid)}</td></tr>
                        }
                    }).collect::<Vec<_>>()}</tbody></table>
                })}
            </Card>
        </div>
    }
}

// ---------------------------------------------------------------------- node

#[component]
pub fn Node() -> impl IntoView {
    let caps = load(api::capabilities);
    let services = load(api::services);
    let assets = load(api::assets);
    let swaps = load(api::swaps);
    let diag = load(api::diagnostics);
    let peers = load(api::peers);
    let (backup, set_backup) = signal(None::<Result<String, String>>);
    let export = move |_| {
        spawn_local(async move {
            let r = api::channel_backup().await.map(|b| b.multi_chan_backup_hex);
            set_backup.set(Some(r.map_err(|e| e.to_string())));
        });
    };
    view! {
        <div class="grid">
            <Card title="Lightning Terminal (litd) services">
                {panel(services, |s: ServicesView| {
                    let tiles = s.sub_servers.into_iter().map(|(name, st)| {
                        let (cls, label) = if st.running { ("up", "running") } else if st.disabled { ("off", "disabled") } else { ("down", "stopped") };
                        view! { <div class=format!("svc {cls}")><div class="svc-name">{name}</div><div class="small">{label}</div><div class="small muted">{st.error}</div></div> }
                    }).collect::<Vec<_>>();
                    view! { <div class="muted small pad-b">{format!("network {}", s.network)}</div><div class="svcs">{tiles}</div> }
                })}
            </Card>
            <Card title="Taproot Assets">
                {panel(assets, |a: AssetsView| {
                    let rows = a.balances.into_iter().map(|b| view! {
                        <div class="row"><span>{b.name}</span><span class="muted small">{b.asset_type}</span><span class="mono">{short(&b.asset_id_hex)}</span><span class="num">{b.balance.to_string()}</span></div>
                    }).collect::<Vec<_>>();
                    let uni = a.universe.map(|u| format!("tapd {} · height {}", u.version, u.block_height)).unwrap_or_default();
                    view! { <div class="rows">{rows}</div><div class="muted small">{uni}</div> }
                })}
            </Card>
            <Card title="Loop swaps">
                {panel(swaps, |s: SwapsView| if s.swaps.is_empty() {
                    view! { <div class="muted">"No swaps yet. Loop Out moves channel balance on-chain (more inbound); Loop In does the reverse."</div> }.into_any()
                } else {
                    let rows = s.swaps.into_iter().map(|w| view! {
                        <div class="row"><span>{w.swap_type}</span><span class="chip">{w.state}</span><span class="num">{sats(w.amount_sat)}</span><span class="muted small">{format!("cost {}", sats(w.cost_sat))}</span></div>
                    }).collect::<Vec<_>>();
                    view! { <div class="rows">{rows}</div> }.into_any()
                })}
            </Card>
            <Card title="Faraday channel insights">
                {panel(diag, |d: DiagnosticsView| {
                    let head = format!("{} · LND {} · height {}", d.node.alias, d.node.version.split(' ').next().unwrap_or(""), d.node.block_height);
                    let body = if d.faraday.available {
                        d.faraday.insights.into_iter().map(|i| view! {
                            <div class="row"><span class="mono">{short(&i.chan_point)}</span>
                            <span class="small">{format!("uptime {:.0}%", i.uptime_ratio.unwrap_or(0.0) * 100.0)}</span>
                            <span class="small muted">{format!("fees {} msat", i.fees_earned_msat)}</span></div>
                        }).collect::<Vec<_>>().into_any()
                    } else {
                        view! { <div class="muted">{d.faraday.error.unwrap_or_default()}</div> }.into_any()
                    };
                    view! { <div class="muted small pad-b">{head}</div><div class="rows">{body}</div> }
                })}
            </Card>
            <Card title="Peers">
                {panel(peers, |p: PeersView| {
                    let rows = p.peers.into_iter().map(|p| view! {
                        <div class="row"><span class="mono">{short(&p.pubkey)}</span><span class="small">{p.address}</span><span class="small muted">{format!("{} ms", p.ping_ms)}</span></div>
                    }).collect::<Vec<_>>();
                    view! { <div class="rows">{rows}</div> }
                })}
            </Card>
            <Card title="Channel backup & capabilities">
                <p class="muted small">"The static channel backup (SCB) lets you recover channel funds with your seed. Keep a fresh copy after every channel change."</p>
                <button class="ghost" on:click=export>"Export SCB"</button>
                {move || backup.get().map(|r| match r {
                    Ok(hex) => {
                        let href = format!("data:text/plain;charset=utf-8,{hex}");
                        view! { <a class="link" href=href download="channel.backup.hex">{format!("Download ({} bytes)", hex.len() / 2)}</a> }.into_any()
                    }
                    Err(e) => view! { <ErrorBox msg=e /> }.into_any(),
                })}
                {panel(caps, |c: rw_core::CapabilitySet| {
                    let chips = c.iter().map(|c| view! { <span class="chip">{format!("{c:?}")}</span> }).collect::<Vec<_>>();
                    view! { <div class="chips">{chips}</div> }
                })}
            </Card>
        </div>
    }
}

// ------------------------------------------------------------------- journal

#[component]
pub fn Journal() -> impl IntoView {
    let intents = load(api::intents);
    view! {
        <div class="grid">
            <Card title="Payment journal">
                <p class="muted small">
                    "Every send is a payment intent, saved before anything leaves the device: "
                    <b>"Authorized → Signing → In flight → Settled"</b>
                    ". After a crash the wallet looks each in-flight payment up on the node by hash (or by transaction label for on-chain) and never sends it again."
                </p>
                {panel(intents, |v: Vec<rw_payments::PaymentIntent>| if v.is_empty() {
                    view! { <div class="muted pad">"No payments yet in this session. Try Send."</div> }.into_any()
                } else {
                    let rows = v.into_iter().rev().map(|i| {
                        let st = format!("{:?}", i.state);
                        let chip = format!("chip {}", st.to_ascii_lowercase());
                        let steps = i.attempts.iter().map(|a| format!("{:?}", a.result)).collect::<Vec<_>>().join(" → ");
                        view! {
                            <tr>
                                <td class="mono">{short(&i.intent_id)}</td>
                                <td>{format!("{:?}", i.kind)}</td>
                                <td><span class=chip>{st}</span></td>
                                <td class="mono">{i.backend_ref.as_deref().map(short).unwrap_or_default()}</td>
                                <td class="small muted">{steps}</td>
                            </tr>
                        }
                    }).collect::<Vec<_>>();
                    view! {
                        <table><thead><tr><th>"Intent"</th><th>"Kind"</th><th>"State"</th><th>"Hash / txid"</th><th>"Attempts"</th></tr></thead>
                        <tbody>{rows}</tbody></table>
                    }.into_any()
                })}
            </Card>
        </div>
    }
}
