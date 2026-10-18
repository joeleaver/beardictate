//! The HTTP agent the downloader uses.
//!
//! ureq's body timeout is a total budget, which a multi-GB download can't
//! have, and with no timeout a stalled connection blocks a read forever (and
//! with it, cancellation). So the agent wraps every connection in
//! [`IdleTimeoutConnector`], which caps each individual socket wait: the
//! download fails with a timeout only when *no* bytes arrive for that long.

use std::fmt;
use std::time::Duration;

use ureq::Agent;
use ureq::unversioned::resolver::DefaultResolver;
use ureq::unversioned::transport::time::Duration as UreqDuration;
use ureq::unversioned::transport::{
    Buffers, ConnectionDetails, Connector, DefaultConnector, NextTimeout, Transport,
};

/// Network settings for [`crate::Downloader`].
#[derive(Debug, Clone)]
pub struct HttpOptions {
    /// Fail a source when no bytes arrive for this long.
    pub idle_timeout: Duration,
    /// Budget for DNS + TCP + TLS setup per request.
    pub connect_timeout: Duration,
    /// Refuse plain http, including redirects to it. Integrity comes from
    /// sha256 regardless; this is defence in depth (and privacy). Tests
    /// against a local server turn it off.
    pub https_only: bool,
    pub user_agent: String,
}

impl Default for HttpOptions {
    fn default() -> Self {
        HttpOptions {
            idle_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(20),
            https_only: true,
            user_agent: format!("beardictate/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

/// Builds the agent: no status-as-error (we inspect statuses ourselves),
/// redirects followed, and the idle-timeout connector.
pub fn agent(opts: &HttpOptions) -> Agent {
    let config = Agent::config_builder()
        .http_status_as_error(false)
        .https_only(opts.https_only)
        .max_redirects(10)
        .timeout_connect(Some(opts.connect_timeout))
        .user_agent(opts.user_agent.as_str())
        .build();
    let connector = DefaultConnector::new().chain(IdleTimeoutConnector { idle: opts.idle_timeout });
    Agent::with_parts(config, connector, DefaultResolver::default())
}

/// Wraps the transport from the default connector chain (TCP, then TLS) so
/// that no single wait exceeds `idle`.
#[derive(Debug)]
struct IdleTimeoutConnector {
    idle: Duration,
}

impl<In: Transport> Connector<In> for IdleTimeoutConnector {
    type Out = IdleTimeoutTransport<In>;

    fn connect(&self, _details: &ConnectionDetails, chained: Option<In>) -> Result<Option<Self::Out>, ureq::Error> {
        Ok(chained.map(|inner| IdleTimeoutTransport { inner, idle: self.idle }))
    }
}

struct IdleTimeoutTransport<T> {
    inner: T,
    idle: Duration,
}

impl<T> fmt::Debug for IdleTimeoutTransport<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdleTimeoutTransport").field("idle", &self.idle).finish_non_exhaustive()
    }
}

impl<T> IdleTimeoutTransport<T> {
    /// The sooner of ureq's own deadline and our idle cap. The reason stays
    /// ureq's when its deadline is sooner, so its errors read right.
    fn cap(&self, timeout: NextTimeout) -> NextTimeout {
        if timeout.after.is_not_happening() || *timeout.after > self.idle {
            NextTimeout { after: UreqDuration::from(self.idle), reason: timeout.reason }
        } else {
            timeout
        }
    }
}

impl<T: Transport> Transport for IdleTimeoutTransport<T> {
    fn buffers(&mut self) -> &mut dyn Buffers {
        self.inner.buffers()
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        let timeout = self.cap(timeout);
        self.inner.transmit_output(amount, timeout)
    }

    fn maybe_await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
        let timeout = self.cap(timeout);
        self.inner.maybe_await_input(timeout)
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
        let timeout = self.cap(timeout);
        self.inner.await_input(timeout)
    }

    fn is_open(&mut self) -> bool {
        self.inner.is_open()
    }

    fn is_tls(&self) -> bool {
        self.inner.is_tls()
    }
}
