use leptos::prelude::*;
use leptos::task::spawn_local;
use rw_core::ApiResult;
use std::future::Future;

/// Bumped to reload every loaded panel ("Refresh" button, after a send).
#[derive(Clone, Copy)]
pub struct Refresh(pub RwSignal<u32>);

pub fn bump() {
    if let Some(Refresh(r)) = use_context::<Refresh>() {
        r.update(|n| *n += 1);
    }
}

/// Load once on mount and again on every refresh.
pub fn load<T, Fut>(fetch: impl Fn() -> Fut + 'static) -> ReadSignal<Option<ApiResult<T>>>
where
    T: Send + Sync + 'static,
    Fut: Future<Output = ApiResult<T>> + 'static,
{
    let epoch = use_context::<Refresh>().map(|r| r.0);
    let (get, set) = signal(None);
    Effect::new(move |_| {
        if let Some(e) = epoch {
            e.track();
        }
        let fut = fetch();
        spawn_local(async move { set.set(Some(fut.await)) });
    });
    get
}

/// Render a loaded value, a spinner while pending, or the error.
pub fn panel<T: Clone + Send + Sync + 'static, V: IntoView + 'static>(
    data: ReadSignal<Option<ApiResult<T>>>,
    f: impl Fn(T) -> V + Send + Sync + 'static,
) -> impl IntoView {
    move || match data.get() {
        None => view! { <div class="muted pad">"Loading…"</div> }.into_any(),
        Some(Err(e)) => view! { <ErrorBox msg=e.to_string() /> }.into_any(),
        Some(Ok(v)) => f(v).into_any(),
    }
}

#[component]
pub fn ErrorBox(msg: String) -> impl IntoView {
    view! { <div class="error" role="alert">{msg}</div> }
}

#[component]
pub fn Card(#[prop(into)] title: String, children: Children) -> impl IntoView {
    view! {
        <section class="card">
            <h2>{title}</h2>
            {children()}
        </section>
    }
}

#[component]
pub fn Stat(
    #[prop(into)] label: String,
    #[prop(into)] value: String,
    #[prop(into, optional)] sub: String,
) -> impl IntoView {
    view! {
        <div class="stat">
            <div class="stat-label">{label}</div>
            <div class="stat-value">{value}</div>
            <div class="stat-sub">{sub}</div>
        </div>
    }
}

/// Local vs remote liquidity of a channel.
#[component]
pub fn Liquidity(local: u64, remote: u64) -> impl IntoView {
    let total = local.saturating_add(remote);
    let l = crate::fmt::pct(local, total);
    view! {
        <div class="liq" title=format!("{} local · {} remote", crate::fmt::sats(local), crate::fmt::sats(remote))>
            <div class="liq-local" style:width=format!("{l:.1}%")></div>
        </div>
    }
}

/// QR code rendered to inline SVG (no network, no canvas).
#[component]
pub fn Qr(data: String) -> impl IntoView {
    let svg = qrcode::QrCode::with_error_correction_level(data.to_uppercase_if_bech32().as_bytes(), qrcode::EcLevel::M)
        .map(|c| {
            c.render::<qrcode::render::svg::Color<'_>>()
                .min_dimensions(220, 220)
                .dark_color(qrcode::render::svg::Color("#0b0d10"))
                .light_color(qrcode::render::svg::Color("#ffffff"))
                .build()
        })
        .unwrap_or_default();
    view! { <div class="qr" inner_html=svg></div> }
}

trait Bech32Upper {
    fn to_uppercase_if_bech32(&self) -> String;
}

impl Bech32Upper for String {
    /// Bech32 strings are case-insensitive; uppercase QR codes are denser (alphanumeric mode).
    fn to_uppercase_if_bech32(&self) -> String {
        let l = self.to_ascii_lowercase();
        if l.starts_with("ln") || l.starts_with("bc1") || l.starts_with("tb1") || l.starts_with("bcrt1") {
            self.to_ascii_uppercase()
        } else {
            self.clone()
        }
    }
}

#[component]
pub fn Copy(text: String) -> impl IntoView {
    let (copied, set_copied) = signal(false);
    let t = text.clone();
    view! {
        <button
            class="ghost small"
            on:click=move |_| {
                if let Some(w) = web_sys::window() {
                    let _ = w.navigator().clipboard().write_text(&t);
                    set_copied.set(true);
                }
            }
        >
            {move || if copied.get() { "Copied" } else { "Copy" }}
        </button>
    }
}
