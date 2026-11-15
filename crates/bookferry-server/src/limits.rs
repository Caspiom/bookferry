//! Teto de conexoes simultaneas, aplicado ANTES do handshake TLS.
//!
//! Sem isso, qualquer host que alcance a porta consegue abrir milhares de sockets e esgotar
//! memoria/descritores do PC. Dois tetos: um global e um por IP de origem (o loopback fica de
//! fora do teto por IP, porque e o proprio app desktop falando com o servidor embutido).

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use std::collections::HashMap;
use std::io;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::Instant;

#[derive(Default)]
struct Counters {
    total: usize,
    per_ip: HashMap<IpAddr, usize>,
}

/// Controla quantas conexoes estao abertas. Compartilhado entre as tarefas de conexao.
pub struct ConnectionLimiter {
    max_total: usize,
    max_per_ip: usize,
    counters: Mutex<Counters>,
}

/// Reserva de uma vaga; devolve a vaga ao ser descartada (fim da conexao, com ou sem erro).
pub struct ConnectionPermit {
    limiter: Arc<ConnectionLimiter>,
    ip: IpAddr,
}

impl ConnectionLimiter {
    pub fn new(max_total: usize, max_per_ip: usize) -> Arc<Self> {
        Arc::new(Self {
            max_total: max_total.max(1),
            max_per_ip: max_per_ip.max(1),
            counters: Mutex::new(Counters::default()),
        })
    }

    /// Tenta reservar uma vaga para `ip`. `None` = teto atingido (a conexao deve ser derrubada).
    pub fn try_acquire(self: &Arc<Self>, ip: IpAddr) -> Option<ConnectionPermit> {
        // `::ffff:1.2.3.4` e `1.2.3.4` sao o mesmo host.
        let ip = ip.to_canonical();
        let mut c = self.counters.lock().unwrap_or_else(|e| e.into_inner());
        if c.total >= self.max_total {
            return None;
        }
        if !ip.is_loopback() {
            let n = c.per_ip.entry(ip).or_insert(0);
            if *n >= self.max_per_ip {
                return None;
            }
            *n += 1;
        }
        c.total += 1;
        drop(c);
        Some(ConnectionPermit {
            limiter: Arc::clone(self),
            ip,
        })
    }

    #[cfg(test)]
    fn open_total(&self) -> usize {
        self.counters.lock().unwrap().total
    }
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        let mut c = self
            .limiter
            .counters
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        c.total = c.total.saturating_sub(1);
        if !self.ip.is_loopback() {
            if let Some(n) = c.per_ip.get_mut(&self.ip) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    c.per_ip.remove(&self.ip);
                }
            }
        }
    }
}

/// Atividade de UMA conexao: quando houve o ultimo byte lido/escrito e quantas requisicoes estao
/// em andamento. O hyper so cobra `header_read_timeout` depois que chega o primeiro byte, entao
/// uma conexao que fecha o TLS e fica muda (ou ociosa entre requisicoes) nunca expiraria sozinha
/// e ocuparia uma vaga do teto para sempre.
pub struct ConnActivity {
    start: Instant,
    last_io_ms: AtomicU64,
    in_flight: AtomicUsize,
}

impl ConnActivity {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            start: Instant::now(),
            last_io_ms: AtomicU64::new(0),
            in_flight: AtomicUsize::new(0),
        })
    }

    fn touch(&self) {
        self.last_io_ms
            .store(self.start.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    fn idle_for(&self) -> Duration {
        let now = self.start.elapsed().as_millis() as u64;
        Duration::from_millis(now.saturating_sub(self.last_io_ms.load(Ordering::Relaxed)))
    }

    /// Resolve quando a conexao ficou `idle` sem trafego E sem requisicao em andamento.
    /// Um rescan demorado (sem I/O) nao conta como ocioso; o SSE escreve keepalive, entao tambem nao.
    pub async fn wait_idle(&self, idle: Duration) {
        loop {
            if self.in_flight.load(Ordering::Relaxed) > 0 {
                tokio::time::sleep(idle).await;
                continue;
            }
            let idle_for = self.idle_for();
            if idle_for >= idle {
                return;
            }
            tokio::time::sleep(idle - idle_for).await;
        }
    }
}

/// Middleware por conexao: marca requisicao em andamento enquanto o handler roda.
pub async fn track_in_flight(
    State(activity): State<Arc<ConnActivity>>,
    req: Request,
    next: Next,
) -> Response {
    struct Guard(Arc<ConnActivity>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.in_flight.fetch_sub(1, Ordering::Relaxed);
            self.0.touch();
        }
    }
    activity.in_flight.fetch_add(1, Ordering::Relaxed);
    let _guard = Guard(activity);
    next.run(req).await
}

/// Embrulha o fluxo TLS e registra cada byte lido ou escrito em `ConnActivity`.
pub struct ActivityStream<S> {
    inner: S,
    activity: Arc<ConnActivity>,
}

impl<S> ActivityStream<S> {
    pub fn new(inner: S, activity: Arc<ConnActivity>) -> Self {
        activity.touch();
        Self { inner, activity }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for ActivityStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let res = Pin::new(&mut self.inner).poll_read(cx, buf);
        if matches!(res, Poll::Ready(Ok(()))) && buf.filled().len() > before {
            self.activity.touch();
        }
        res
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for ActivityStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let res = Pin::new(&mut self.inner).poll_write(cx, data);
        if matches!(res, Poll::Ready(Ok(n)) if n > 0) {
            self.activity.touch();
        }
        res
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn per_ip_cap_blocks_one_host_but_not_another() {
        let l = ConnectionLimiter::new(100, 2);
        let a1 = l.try_acquire(ip("192.168.1.10")).unwrap();
        let _a2 = l.try_acquire(ip("192.168.1.10")).unwrap();
        assert!(l.try_acquire(ip("192.168.1.10")).is_none());
        assert!(l.try_acquire(ip("192.168.1.11")).is_some());
        drop(a1);
        assert!(l.try_acquire(ip("192.168.1.10")).is_some());
    }

    #[test]
    fn global_cap_applies_to_all_hosts_and_permits_are_returned() {
        let l = ConnectionLimiter::new(3, 10);
        let p1 = l.try_acquire(ip("10.0.0.1")).unwrap();
        let _p2 = l.try_acquire(ip("10.0.0.2")).unwrap();
        let _p3 = l.try_acquire(ip("10.0.0.3")).unwrap();
        assert!(l.try_acquire(ip("10.0.0.4")).is_none());
        drop(p1);
        assert_eq!(l.open_total(), 2);
        assert!(l.try_acquire(ip("10.0.0.4")).is_some());
    }

    #[test]
    fn loopback_ignores_per_ip_cap_but_not_global_cap() {
        let l = ConnectionLimiter::new(4, 1);
        let _a = l.try_acquire(ip("127.0.0.1")).unwrap();
        let _b = l.try_acquire(ip("127.0.0.1")).unwrap();
        let _c = l.try_acquire(ip("::1")).unwrap();
        let _d = l.try_acquire(ip("127.0.0.1")).unwrap();
        assert!(l.try_acquire(ip("127.0.0.1")).is_none());
    }

    #[tokio::test]
    async fn idle_connection_expires_but_in_flight_request_and_traffic_keep_it_alive() {
        let idle = Duration::from_millis(300);

        // Sem trafego e sem requisicao: expira.
        let a = ConnActivity::new();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), a.wait_idle(idle))
                .await
                .is_ok()
        );

        // Com requisicao em andamento (ex.: rescan longo sem I/O): NAO expira.
        let b = ConnActivity::new();
        b.in_flight.fetch_add(1, Ordering::Relaxed);
        assert!(
            tokio::time::timeout(Duration::from_millis(900), b.wait_idle(idle))
                .await
                .is_err()
        );

        // Trafego adia a expiracao: sem ele expiraria em 300 ms; tocando aos 200 ms,
        // so expira ~300 ms depois do toque.
        let c = ConnActivity::new();
        let waiter = c.wait_idle(idle);
        tokio::pin!(waiter);
        tokio::time::sleep(Duration::from_millis(200)).await;
        c.touch();
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut waiter)
                .await
                .is_err()
        );
        assert!(tokio::time::timeout(Duration::from_secs(2), &mut waiter)
            .await
            .is_ok());
    }

    #[test]
    fn ipv4_mapped_ipv6_counts_as_the_same_host() {
        let l = ConnectionLimiter::new(100, 1);
        let _a = l.try_acquire(ip("192.168.1.10")).unwrap();
        assert!(l.try_acquire(ip("::ffff:192.168.1.10")).is_none());
    }
}
