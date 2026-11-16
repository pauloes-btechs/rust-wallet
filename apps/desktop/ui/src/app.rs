use crate::api::{self, Mode};
use crate::components::*;
use crate::pages::*;
use leptos::prelude::*;
use leptos::task::spawn_local;
use rw_core::views::{Phase, StateView};
use rw_core::{Capability, CapabilitySet, Network};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tab {
    Overview,
    Send,
    Receive,
    Channels,
    Activity,
    Node,
    Journal,
}

const TABS: &[(Tab, &str, Option<Capability>)] = &[
    (Tab::Overview, "Overview", None),
    (Tab::Send, "Send", Some(Capability::LightningSend)),
    (Tab::Receive, "Receive", Some(Capability::LightningReceive)),
    (Tab::Channels, "Channels", Some(Capability::Channels)),
    (Tab::Activity, "Activity", None),
    (Tab::Node, "Node", Some(Capability::Services)),
    (Tab::Journal, "Journal", None),
];

#[component]
pub fn App() -> impl IntoView {
    let refresh = RwSignal::new(0u32);
    provide_context(Refresh(refresh));
    let (net, set_net) = signal(None::<Network>);
    provide_context(Net(net));
    let (tab, set_tab) = signal(Tab::Overview);
    let (conn, set_conn) = signal(None::<Result<api::ConnectionInfo, String>>);
    let (caps, set_caps) = signal(CapabilitySet::default());
    let (state, set_state) = signal(None::<StateView>);

    // Connection + node state, re-read on every refresh.
    Effect::new(move |_| {
        refresh.track();
        spawn_local(async move {
            let c = api::connection_info().await.map_err(|e| e.to_string());
            let connected = c.as_ref().map(|c| c.kind != "none").unwrap_or(false);
            set_conn.set(Some(c));
            if !connected {
                return;
            }
            if let Ok(s) = api::state().await {
                set_net.set(s.info.as_ref().and_then(|i| i.network()));
                set_state.set(Some(s));
            }
            if let Ok(c) = api::capabilities().await {
                set_caps.set(c);
            }
        });
    });

    let demo = api::mode() == Mode::Demo;
    let phase = move || state.get().map(|s| s.phase);
    let needs_setup = move || matches!(conn.get(), Some(Ok(c)) if c.kind == "none");
    let wallet_gate = move || matches!(phase(), Some(Phase::Missing | Phase::Locked));

    view! {
        <Show when=move || demo>
            <div class="banner" role="note">
                <b>"Demo"</b>
                {format!(
                    " · replaying a real litd {} session recorded on regtest ({}). No network calls, no keys, no real bitcoin. Payments run through the real Rust payment engine.",
                    rw_replay::meta().litd,
                    rw_replay::meta().recorded_at.get(..10).unwrap_or(""),
                )}
            </div>
        </Show>
        <header class="top">
            <div class="brand"><span class="logo">"⚡"</span>"rust-wallet"</div>
            <div class="status">
                {move || state.get().and_then(|s| s.info).map(|i| view! {
                    <span class="chip net">{i.chains.first().map(|c| c.network.clone()).unwrap_or_default()}</span>
                    <span class="muted small">{format!("{} · block {}", i.alias, i.block_height)}</span>
                    <span class=if i.synced_to_chain { "dot ok" } else { "dot warn" } title="chain sync"></span>
                })}
                <button class="ghost small" title="Refresh" on:click=move |_| refresh.update(|n| *n += 1)>"↻"</button>
                <Show when=move || !demo && matches!(conn.get(), Some(Ok(c)) if c.kind != "none")>
                    <button class="ghost small" title="Disconnect (stops a local node)" on:click=move |_| {
                        spawn_local(async move {
                            let _ = api::disconnect().await;
                            set_state.set(None);
                            refresh.update(|n| *n += 1);
                        });
                    }>"Disconnect"</button>
                </Show>
            </div>
        </header>
        <Show
            when=move || !needs_setup() && !wallet_gate()
            fallback=move || view! {
                {move || if needs_setup() { view! { <Setup /> }.into_any() } else { view! { <WalletGate phase=phase().unwrap_or(Phase::Undetermined) /> }.into_any() }}
            }
        >
            <nav class="tabs">
                {move || {
                    let c = caps.get();
                    TABS.iter()
                        .filter(|(_, _, need)| need.is_none_or(|n| c.has(n)))
                        .map(|&(t, label, _)| view! {
                            <button class:active=move || tab.get() == t on:click=move |_| set_tab.set(t)>{label}</button>
                        })
                        .collect::<Vec<_>>()
                }}
            </nav>
            <main>
                {move || match tab.get() {
                    Tab::Overview => view! { <Overview /> }.into_any(),
                    Tab::Send => view! { <Send /> }.into_any(),
                    Tab::Receive => view! { <Receive /> }.into_any(),
                    Tab::Channels => view! { <Channels /> }.into_any(),
                    Tab::Activity => view! { <Activity /> }.into_any(),
                    Tab::Node => view! { <Node /> }.into_any(),
                    Tab::Journal => view! { <Journal /> }.into_any(),
                }}
            </main>
        </Show>
        <footer class="foot muted small">
            "rust-wallet · Rust + Leptos + Tauri over Lightning Labs litd · "
            <a class="link" href="https://github.com/pauloes-btechs/rust-wallet" target="_blank" rel="noopener">"source"</a>
        </footer>
    }
}

/// Desktop first run: connect to an existing node, or run one locally.
#[component]
fn Setup() -> impl IntoView {
    let (endpoint, set_endpoint) = signal("127.0.0.1:10009".to_string());
    let (cert, set_cert) = signal(String::new());
    let (mac, set_mac) = signal(String::new());
    let (peers, set_peers) = signal("127.0.0.1:18444".to_string());
    let (network, set_network) = signal("regtest".to_string());
    let (err, set_err) = signal(None::<String>);
    let (busy, set_busy) = signal(false);

    let connect = move |_| {
        set_busy.set(true);
        let req = api::RemoteNode { endpoint: endpoint.get(), tls_cert_path: cert.get(), macaroon_path: mac.get() };
        spawn_local(async move {
            match api::connect_remote(&req).await {
                Ok(_) => bump(),
                Err(e) => set_err.set(Some(e.to_string())),
            }
            set_busy.set(false);
        });
    };
    let local = move |_| {
        set_busy.set(true);
        let req = api::SidecarRequest {
            network: network.get(),
            neutrino_peers: peers.get().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        };
        spawn_local(async move {
            match api::start_sidecar(&req).await {
                Ok(_) => bump(),
                Err(e) => set_err.set(Some(e.to_string())),
            }
            set_busy.set(false);
        });
    };

    view! {
        <main class="grid setup">
            <Card title="Connect to your node">
                <p class="muted small">"LND or Lightning Terminal (litd). The certificate is pinned exactly; the macaroon never leaves this computer."</p>
                <label class="field"><span>"gRPC endpoint"</span><input class="mono" prop:value=move || endpoint.get() on:input=move |ev| set_endpoint.set(event_target_value(&ev)) /></label>
                <label class="field"><span>"tls.cert path"</span><input class="mono" placeholder="~/.lnd/tls.cert" prop:value=move || cert.get() on:input=move |ev| set_cert.set(event_target_value(&ev)) /></label>
                <label class="field"><span>"admin.macaroon path"</span><input class="mono" placeholder="~/.lnd/data/chain/bitcoin/regtest/admin.macaroon" prop:value=move || mac.get() on:input=move |ev| set_mac.set(event_target_value(&ev)) /></label>
                <button class="primary" disabled=move || busy.get() on:click=connect>"Connect"</button>
            </Card>
            <Card title="Run a node on this computer">
                <p class="muted small">"Starts the bundled litd v0.17.8 (hash-verified) on loopback only, with a Neutrino light client. Regtest and signet only in this build."</p>
                <label class="field"><span>"Network"</span>
                    <select on:change=move |ev| set_network.set(event_target_value(&ev))>
                        <option value="regtest" selected=true>"regtest"</option>
                        <option value="signet">"signet"</option>
                    </select>
                </label>
                <label class="field"><span>"Neutrino peers (comma separated)"</span><input class="mono" prop:value=move || peers.get() on:input=move |ev| set_peers.set(event_target_value(&ev)) /></label>
                <button class="primary" disabled=move || busy.get() on:click=local>"Start local node"</button>
            </Card>
            {move || err.get().map(|e| view! { <ErrorBox msg=e /> })}
        </main>
    }
}

/// Create (with seed backup) or unlock the node's wallet.
#[component]
fn WalletGate(phase: Phase) -> impl IntoView {
    let (password, set_password) = signal(String::new());
    let (seed, set_seed) = signal(None::<Vec<String>>);
    let (confirmed, set_confirmed) = signal(false);
    let (err, set_err) = signal(None::<String>);

    let unlock = move |_| {
        let pw = password.get();
        spawn_local(async move {
            match api::unlock_wallet(&pw).await {
                Ok(()) => bump(),
                Err(e) => set_err.set(Some(e.to_string())),
            }
        });
    };
    let gen = move |_| {
        spawn_local(async move {
            match api::gen_seed().await {
                Ok(words) => set_seed.set(Some(words)),
                Err(e) => set_err.set(Some(e.to_string())),
            }
        });
    };
    let create = move |_| {
        let (Some(words), pw) = (seed.get(), password.get()) else { return };
        if pw.len() < 8 {
            set_err.set(Some("use a password of at least 8 characters".into()));
            return;
        }
        spawn_local(async move {
            match api::init_wallet(&api::InitWalletRequest { password: pw, mnemonic: words }).await {
                Ok(()) => {
                    set_seed.set(None);
                    bump();
                }
                Err(e) => set_err.set(Some(e.to_string())),
            }
        });
    };

    let pw_field = move || {
        view! {
            <label class="field"><span>"Wallet password"</span>
                <input type="password" autocomplete="new-password" prop:value=move || password.get() on:input=move |ev| set_password.set(event_target_value(&ev)) /></label>
        }
    };

    view! {
        <main class="grid narrow">
            {match phase {
                Phase::Locked => view! {
                    <Card title="Unlock wallet">
                        {pw_field}
                        <button class="primary" on:click=unlock>"Unlock"</button>
                    </Card>
                }.into_any(),
                _ => view! {
                    <Card title="Create wallet">
                        <p class="muted small">"Your node needs a wallet. You will see a 24-word seed once: write it down. With it and a channel backup you can recover your funds."</p>
                        {move || match seed.get() {
                            None => view! { <button class="primary" on:click=gen>"Generate seed"</button> }.into_any(),
                            Some(words) => view! {
                                <ol class="seed">{words.into_iter().map(|w| view! { <li>{w}</li> }).collect::<Vec<_>>()}</ol>
                                <label class="check"><input type="checkbox" on:change=move |ev| set_confirmed.set(event_target_checked(&ev)) />"I wrote the 24 words down, in order"</label>
                                {pw_field}
                                <button class="primary" disabled=move || !confirmed.get() on:click=create>"Create wallet"</button>
                            }.into_any(),
                        }}
                    </Card>
                }.into_any(),
            }}
            {move || err.get().map(|e| view! { <ErrorBox msg=e /> })}
        </main>
    }
}
