<script lang="ts">
  type Health = {
    status?: string;
    service?: string;
    outbound_available_permits?: number;
    max_outbound_concurrency?: number;
    circuit_breaker_state?: string;
    circuit_breaker_failures?: number;
    last_successful_origin_call_unix_secs?: number | null;
  };

  type Metrics = {
    rolling_window_seconds?: number;
    total_requests?: number;
    cache_hits?: number;
    stale_responses?: number;
    origin_errors?: number;
    cache_hit_rate?: number;
    stale_response_rate?: number;
    origin_error_rate?: number;
    p95_latency_ms?: number;
  };

  type MetricsPoint = {
    observed_at_unix_secs: number;
    total_requests: number;
    cache_hit_rate: number;
    stale_response_rate: number;
    p95_latency_ms: number;
  };

  type MetricsTimeseries = {
    bucket_seconds?: number;
    points?: MetricsPoint[];
  };

  type ApiEnvelope = {
    data?: unknown;
    cached?: boolean;
    stale?: boolean;
    source?: string;
    error?: { code?: string; message?: string };
  };

  type Tool = "entity" | "search" | "sparql";

  const endpointOptions = [
    { label: "Default DBpedia", value: "" },
    { label: "Main DBpedia", value: "https://dbpedia.org/sparql" },
    { label: "German DBpedia", value: "https://de.dbpedia.org/sparql" },
    { label: "French DBpedia", value: "https://fr.dbpedia.org/sparql" }
  ];

  const languageOptions = [
    { label: "English", value: "en" },
    { label: "German", value: "de" },
    { label: "French", value: "fr" },
    { label: "Amharic", value: "am" },
    { label: "Spanish", value: "es" }
  ];

  let apiBase = "";
  let health: Health = {};
  let metrics: Metrics = {};
  let timeseries: MetricsTimeseries = {};
  let responsePayload: unknown = { status: "ready", hint: "Run a request to inspect the gateway response." };
  let lastUpdated = "Not refreshed yet";
  let loadingDashboard = false;
  let entityLoading = false;
  let searchLoading = false;
  let sparqlLoading = false;
  let entityId = "Albert_Einstein";
  let entityEndpoint = "";
  let entityLanguage = "en";
  let searchQuery = "Ethiopia";
  let searchEndpoint = "";
  let searchLanguage = "en";
  let sparqlEndpoint = "";
  let sparqlQuery = "SELECT * WHERE { ?s ?p ?o } LIMIT 5";
  let entitySource = "idle";
  let searchSource = "idle";
  let sparqlSource = "idle";
  let activeTool: Tool = "entity";
  let copyState = "Copy JSON";

  $: isHealthy = health.status === "ok";
  $: serviceStatus = health.status ?? "waiting";
  $: breakerLabel = health.circuit_breaker_state
    ? health.circuit_breaker_state
    : "unknown";
  $: permitsLabel =
    health.outbound_available_permits !== undefined &&
    health.max_outbound_concurrency !== undefined
      ? health.outbound_available_permits + "/" + health.max_outbound_concurrency
      : "—";
  $: historyPoints = timeseries.points ?? [];
  $: historyPolyline = chartPolyline(historyPoints);
  $: historyArea = historyPolyline ? historyPolyline + " 100,100 0,100" : "";
  $: historyMax = Math.max(1, ...historyPoints.map((point) => point.total_requests));
  $: responseText = pretty(responsePayload);
  $: responseBytes = new TextEncoder().encode(responseText).length;
  $: responseSource = sourceFrom(responsePayload as ApiEnvelope);
  $: sourceTitle =
    responseSource === "cache"
      ? "Served from Redis cache"
      : responseSource === "stale"
        ? "Stale fallback served"
        : responseSource === "origin"
          ? "Fresh response from DBpedia"
          : responseSource === "error"
            ? "Request needs attention"
            : "Response inspector";

  function apiUrl(path: string) {
    const base = apiBase.trim().replace(/\/$/, "");
    return base + path;
  }

  function withEndpoint(path: string, endpoint: string, language?: string) {
    const params = new URLSearchParams();
    if (endpoint) params.set("endpoint", endpoint);
    if (language) params.set("lang", language);
    const query = params.toString();
    if (!query) return path;
    return path + (path.includes("?") ? "&" : "?") + query;
  }

  function percent(value: number | undefined) {
    if (!Number.isFinite(value)) return "0%";
    return Math.round((value ?? 0) * 100) + "%";
  }

  function number(value: number | undefined) {
    return (value ?? 0).toLocaleString();
  }

  function pretty(payload: unknown) {
    return JSON.stringify(payload, null, 2);
  }

  function chartPolyline(points: MetricsPoint[]) {
    if (!points.length) return "";
    const max = Math.max(1, ...points.map((item) => item.total_requests));
    return points
      .map((point, index) => {
        const x = points.length === 1 ? 50 : (index / (points.length - 1)) * 100;
        const y = 100 - (point.total_requests / max) * 82 - 9;
        return x.toFixed(2) + "," + y.toFixed(2);
      })
      .join(" ");
  }

  function formatBucket(unixSeconds: number) {
    return new Date(unixSeconds * 1000).toLocaleTimeString([], {
      hour: "2-digit",
      minute: "2-digit"
    });
  }

  function updateResponse(payload: unknown) {
    responsePayload = payload;
    lastUpdated = new Date().toLocaleTimeString([], {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit"
    });
    copyState = "Copy JSON";
  }

  function sourceFrom(payload: ApiEnvelope) {
    if (payload?.error) return "error";
    if (payload?.stale) return "stale";
    return payload?.source ?? "idle";
  }

  function sourceLabel(source: string) {
    if (source === "origin") return "DBpedia origin";
    if (source === "cache") return "Redis cache";
    if (source === "stale") return "stale fallback";
    if (source === "error") return "request error";
    if (source === "loading") return "running";
    return "ready";
  }

  async function requestJson<T>(path: string, init?: RequestInit): Promise<T> {
    const response = await fetch(apiUrl(path), {
      headers: {
        "content-type": "application/json",
        ...(init?.headers ?? {})
      },
      ...init
    });
    const text = await response.text();
    const payload = text ? JSON.parse(text) : {};
    if (!response.ok) {
      throw Object.assign(new Error(payload?.error?.message ?? response.statusText), {
        payload,
        status: response.status
      });
    }
    return payload as T;
  }

  async function refreshDashboard() {
    loadingDashboard = true;
    try {
      const [nextHealth, nextMetrics, nextTimeseries] = await Promise.all([
        requestJson<Health>("/v1/health"),
        requestJson<Metrics>("/v1/metrics/summary"),
        requestJson<MetricsTimeseries>("/v1/metrics/timeseries")
      ]);
      health = nextHealth;
      metrics = nextMetrics;
      timeseries = nextTimeseries;
      lastUpdated = new Date().toLocaleTimeString([], {
        hour: "2-digit",
        minute: "2-digit",
        second: "2-digit"
      });
    } catch (error) {
      updateResponse((error as { payload?: unknown }).payload ?? { error: String(error) });
      health = { status: "error", circuit_breaker_state: "unknown" };
    } finally {
      loadingDashboard = false;
    }
  }

  async function runEntity() {
    if (!entityId.trim()) return;
    entityLoading = true;
    entitySource = "loading";
    try {
      const path = withEndpoint(
        "/v1/entity/" + encodeURIComponent(entityId.trim()),
        entityEndpoint,
        entityLanguage
      );
      const payload = await requestJson<ApiEnvelope>(path);
      entitySource = sourceFrom(payload);
      updateResponse(payload);
      await refreshDashboard();
    } catch (error) {
      const payload = (error as { payload?: ApiEnvelope }).payload ?? {
        error: { message: String(error) }
      };
      entitySource = "error";
      updateResponse(payload);
    } finally {
      entityLoading = false;
    }
  }

  async function runSearch() {
    if (!searchQuery.trim()) return;
    searchLoading = true;
    searchSource = "loading";
    try {
      const path = withEndpoint(
        "/v1/search?q=" + encodeURIComponent(searchQuery.trim()),
        searchEndpoint,
        searchLanguage
      );
      const payload = await requestJson<ApiEnvelope>(path);
      searchSource = sourceFrom(payload);
      updateResponse(payload);
      await refreshDashboard();
    } catch (error) {
      const payload = (error as { payload?: ApiEnvelope }).payload ?? {
        error: { message: String(error) }
      };
      searchSource = "error";
      updateResponse(payload);
    } finally {
      searchLoading = false;
    }
  }

  async function runSparql() {
    if (!sparqlQuery.trim()) return;
    sparqlLoading = true;
    sparqlSource = "loading";
    const body: { query: string; endpoint?: string } = {
      query: sparqlQuery.trim()
    };
    if (sparqlEndpoint) body.endpoint = sparqlEndpoint;

    try {
      const payload = await requestJson<ApiEnvelope>("/v1/sparql", {
        method: "POST",
        body: JSON.stringify(body)
      });
      sparqlSource = sourceFrom(payload);
      updateResponse(payload);
      await refreshDashboard();
    } catch (error) {
      const payload = (error as { payload?: ApiEnvelope }).payload ?? {
        error: { message: String(error) }
      };
      sparqlSource = "error";
      updateResponse(payload);
    } finally {
      sparqlLoading = false;
    }
  }

  function chooseExample(tool: Tool, value: string) {
    activeTool = tool;
    if (tool === "entity") entityId = value;
    if (tool === "search") searchQuery = value;
    if (tool === "sparql") sparqlQuery = value;
    document.getElementById("query-lab")?.scrollIntoView({ behavior: "smooth", block: "start" });
  }

  async function copyResponse() {
    try {
      await navigator.clipboard.writeText(responseText);
      copyState = "Copied";
      window.setTimeout(() => (copyState = "Copy JSON"), 1600);
    } catch {
      copyState = "Copy unavailable";
    }
  }

  function focusLab() {
    document.getElementById("query-lab")?.scrollIntoView({ behavior: "smooth", block: "start" });
  }

  refreshDashboard();
</script>

<svelte:head>
  <title>KGProxy / Edge observatory</title>
  <meta
    name="description"
    content="Explore DBpedia through KGProxy's cache, routing, and reliability controls."
  />
</svelte:head>

<div class="app-shell">
  <aside class="sidebar">
    <a class="brand" href="#overview" aria-label="KGProxy overview">
      <span class="brand-mark" aria-hidden="true">
        <svg viewBox="0 0 48 48" fill="none">
          <path d="M8 9h14v8H16v14h6v8H8V9Z" fill="currentColor" />
          <path d="M27 9h13v8h-5v14h5v8H27V9Z" fill="currentColor" opacity=".52" />
          <path d="M17 20h14v8H17v-8Z" fill="#F4C95D" />
        </svg>
      </span>
      <span>
        <strong>KGProxy</strong>
        <small>edge observatory</small>
      </span>
    </a>

    <nav class="side-nav" aria-label="Dashboard sections">
      <a class="nav-item active" href="#overview">
        <span class="nav-icon">◈</span>
        Overview
      </a>
      <a class="nav-item" href="#traffic">
        <span class="nav-icon">⌁</span>
        Traffic pulse
      </a>
      <a class="nav-item" href="#query-lab">
        <span class="nav-icon">⌘</span>
        Query lab
      </a>
      <a class="nav-item" href="#response">
        <span class="nav-icon">↳</span>
        Response
      </a>
    </nav>

    <div class="sidebar-note">
      <span class="note-rule"></span>
      <p>DBpedia access with a memory.</p>
      <small>Cache · limits · fallbacks</small>
    </div>

    <div class="sidebar-footer">
      <span class:offline={!isHealthy} class="signal-dot"></span>
      <span>{isHealthy ? "Gateway online" : "Checking gateway"}</span>
    </div>
  </aside>

  <div class="workspace">
    <header class="topbar">
      <div class="breadcrumb">
        <span>KGProxy</span>
        <span class="breadcrumb-slash">/</span>
        <strong>Operations</strong>
      </div>

      <div class="topbar-actions">
        <label class="api-input">
          <span>API</span>
          <input bind:value={apiBase} placeholder="same origin" aria-label="API base URL" />
        </label>
        <button class="icon-button" type="button" on:click={refreshDashboard} disabled={loadingDashboard} aria-label="Refresh dashboard" title="Refresh dashboard">
          <svg viewBox="0 0 24 24" aria-hidden="true" class:spin={loadingDashboard}>
            <path d="M20 11a8 8 0 0 0-14.8-3.8L3 10m0 0V5m0 5h5M4 13a8 8 0 0 0 14.8 3.8L21 14m0 0v5m0-5h-5" />
          </svg>
        </button>
        <span class:offline={!isHealthy} class="top-status">
          <span class="status-pip"></span>
          {isHealthy ? "Live" : serviceStatus}
        </span>
      </div>
    </header>

    <main>
      <section id="overview" class="hero-section">
        <div class="hero-copy">
          <div class="eyebrow"><span class="eyebrow-mark"></span> DBpedia reliability gateway</div>
          <h1>DBpedia,<br /><em>with a safer edge.</em></h1>
          <p class="hero-lede">
            A transparent control room for cached lookups, language-aware routing,
            and graceful origin failures.
          </p>
          <div class="hero-actions">
            <button class="primary-button" type="button" on:click={focusLab}>
              Open query lab
              <span>↗</span>
            </button>
            <a class="text-button" href="#traffic">See traffic pulse <span>↓</span></a>
          </div>
        </div>

        <div class="route-card" aria-label="KGProxy request path">
          <div class="route-card-top">
            <span class="mono-label">REQUEST PATH / LIVE</span>
            <span class="route-time">{lastUpdated}</span>
          </div>
          <div class="route-map">
            <div class="route-line"><span class="route-travel"></span></div>
            <div class="route-node client-node">
              <span class="node-glyph">⌁</span>
              <strong>Client</strong>
              <small>request</small>
            </div>
            <div class="route-node cache-node">
              <span class="node-glyph">◌</span>
              <strong>Redis</strong>
              <small>cache layer</small>
            </div>
            <div class="route-node origin-node">
              <span class="node-glyph">✦</span>
              <strong>DBpedia</strong>
              <small>origin</small>
            </div>
          </div>
          <div class="route-caption">
            <span class="caption-dot"></span>
            <span>Requests stay observable from edge to origin.</span>
          </div>
        </div>
      </section>

      <section class="metric-grid" aria-label="Gateway metrics">
        <article class="metric-card metric-status">
          <div class="metric-top"><span class="metric-label">Gateway state</span><span class="metric-index">01</span></div>
          <div class="metric-value status-value"><span class:offline={!isHealthy} class="status-pip large"></span>{serviceStatus}</div>
          <div class="metric-foot"><span>breaker</span><strong>{breakerLabel}</strong></div>
        </article>
        <article class="metric-card metric-requests">
          <div class="metric-top"><span class="metric-label">Requests / 24h</span><span class="metric-index">02</span></div>
          <div class="metric-value">{number(metrics.total_requests)}</div>
          <div class="metric-foot"><span>latest bucket</span><strong>{historyPoints.length ? number(historyPoints[historyPoints.length - 1].total_requests) : "—"}</strong></div>
        </article>
        <article class="metric-card metric-cache">
          <div class="metric-top"><span class="metric-label">Cache hit rate</span><span class="metric-index">03</span></div>
          <div class="metric-value">{percent(metrics.cache_hit_rate)}</div>
          <div class="metric-foot"><span>Redis hits</span><strong>{number(metrics.cache_hits)}</strong></div>
        </article>
        <article class="metric-card metric-latency">
          <div class="metric-top"><span class="metric-label">P95 latency</span><span class="metric-index">04</span></div>
          <div class="metric-value">{number(metrics.p95_latency_ms)}<small>ms</small></div>
          <div class="metric-foot"><span>permits</span><strong>{permitsLabel}</strong></div>
        </article>
      </section>

      <section id="traffic" class="content-section">
        <div class="section-heading">
          <div>
            <div class="eyebrow">Signal over time</div>
            <h2>Traffic pulse</h2>
          </div>
          <div class="heading-meta">
            <span class="legend"><i class="legend-line"></i> request volume</span>
            <span class="mono-label">LAST 24 HOURS</span>
          </div>
        </div>

        <div class="traffic-panel">
          {#if historyPoints.length}
            <div class="chart-wrap">
              <div class="chart-y-labels"><span>{number(historyMax)}</span><span>{number(Math.round(historyMax / 2))}</span><span>0</span></div>
              <svg viewBox="0 0 100 100" preserveAspectRatio="none" role="img" aria-label="Request volume over time">
                <line class="chart-grid" x1="0" y1="9" x2="100" y2="9" />
                <line class="chart-grid" x1="0" y1="50" x2="100" y2="50" />
                <line class="chart-grid" x1="0" y1="91" x2="100" y2="91" />
                <polygon class="chart-area" points={historyArea} />
                <polyline class="chart-line" points={historyPolyline} />
              </svg>
            </div>
            <div class="chart-footer">
              <span>{formatBucket(historyPoints[0].observed_at_unix_secs)}</span>
              <strong>{number(historyPoints[historyPoints.length - 1].total_requests)} requests in latest bucket</strong>
              <span>{formatBucket(historyPoints[historyPoints.length - 1].observed_at_unix_secs)}</span>
            </div>
          {:else}
            <div class="empty-state">
              <span class="empty-glyph">⌁</span>
              <strong>No traffic history yet</strong>
              <span>Run a lookup or search below to seed the pulse.</span>
            </div>
          {/if}
        </div>
      </section>

      <section id="query-lab" class="content-section">
        <div class="section-heading">
          <div>
            <div class="eyebrow">Try the edge</div>
            <h2>Query lab</h2>
          </div>
          <span class="section-description">Use real requests to see cache and origin behavior.</span>
        </div>

        <div class="lab-grid">
          <div class="lab-panel">
            <div class="lab-tabs" role="tablist" aria-label="Query tools">
              <button class:active={activeTool === "entity"} type="button" role="tab" aria-selected={activeTool === "entity"} on:click={() => (activeTool = "entity")}>
                <span>01</span> Entity
              </button>
              <button class:active={activeTool === "search"} type="button" role="tab" aria-selected={activeTool === "search"} on:click={() => (activeTool = "search")}>
                <span>02</span> Search
              </button>
              <button class:active={activeTool === "sparql"} type="button" role="tab" aria-selected={activeTool === "sparql"} on:click={() => (activeTool = "sparql")}>
                <span>03</span> SPARQL
              </button>
            </div>

            {#if activeTool === "entity"}
              <form class="tool-form" on:submit|preventDefault={runEntity}>
                <div class="tool-heading">
                  <div><span class="tool-kicker">GET /v1/entity/:id</span><h3>Look up a DBpedia entity</h3></div>
                  <span class:state-origin={entitySource === "origin"} class:state-cache={entitySource === "cache"} class:state-stale={entitySource === "stale"} class:state-error={entitySource === "error"} class:state-loading={entitySource === "loading"} class="source-badge">{sourceLabel(entitySource)}</span>
                </div>
                <label class="field wide-field"><span>Entity ID</span><input bind:value={entityId} autocomplete="off" placeholder="Albert_Einstein" /></label>
                <div class="field-row">
                  <label class="field"><span>Endpoint</span><select bind:value={entityEndpoint}>{#each endpointOptions as option}<option value={option.value}>{option.label}</option>{/each}</select></label>
                  <label class="field"><span>Language</span><select bind:value={entityLanguage}>{#each languageOptions as option}<option value={option.value}>{option.label}</option>{/each}</select></label>
                </div>
                <div class="form-footer"><span class="field-hint">Cache keys include endpoint + language.</span><button class="run-button" type="submit" disabled={entityLoading}>{entityLoading ? "Running..." : "Run lookup"} <span>↗</span></button></div>
              </form>
            {:else if activeTool === "search"}
              <form class="tool-form" on:submit|preventDefault={runSearch}>
                <div class="tool-heading">
                  <div><span class="tool-kicker">GET /v1/search</span><h3>Search across DBpedia</h3></div>
                  <span class:state-origin={searchSource === "origin"} class:state-cache={searchSource === "cache"} class:state-stale={searchSource === "stale"} class:state-error={searchSource === "error"} class:state-loading={searchSource === "loading"} class="source-badge">{sourceLabel(searchSource)}</span>
                </div>
                <label class="field wide-field"><span>Search query</span><input bind:value={searchQuery} autocomplete="off" placeholder="Ethiopia" /></label>
                <div class="field-row">
                  <label class="field"><span>Endpoint</span><select bind:value={searchEndpoint}>{#each endpointOptions as option}<option value={option.value}>{option.label}</option>{/each}</select></label>
                  <label class="field"><span>Language</span><select bind:value={searchLanguage}>{#each languageOptions as option}<option value={option.value}>{option.label}</option>{/each}</select></label>
                </div>
                <div class="form-footer"><span class="field-hint">Language-aware routing stays DBpedia-only.</span><button class="run-button" type="submit" disabled={searchLoading}>{searchLoading ? "Running..." : "Run search"} <span>↗</span></button></div>
              </form>
            {:else}
              <form class="tool-form" on:submit|preventDefault={runSparql}>
                <div class="tool-heading">
                  <div><span class="tool-kicker">POST /v1/sparql</span><h3>Send a raw SPARQL query</h3></div>
                  <span class:state-origin={sparqlSource === "origin"} class:state-cache={sparqlSource === "cache"} class:state-stale={sparqlSource === "stale"} class:state-error={sparqlSource === "error"} class:state-loading={sparqlSource === "loading"} class="source-badge">{sourceLabel(sparqlSource)}</span>
                </div>
                <label class="field wide-field"><span>Endpoint</span><select bind:value={sparqlEndpoint}>{#each endpointOptions as option}<option value={option.value}>{option.label}</option>{/each}</select></label>
                <label class="field wide-field"><span>SPARQL query</span><textarea bind:value={sparqlQuery} spellcheck="false" placeholder="SELECT * WHERE &#123; ?s ?p ?o &#125; LIMIT 5"></textarea></label>
                <div class="form-footer"><span class="field-hint">Raw queries accept an optional DBpedia endpoint.</span><button class="run-button" type="submit" disabled={sparqlLoading}>{sparqlLoading ? "Running..." : "Run query"} <span>↗</span></button></div>
              </form>
            {/if}
          </div>

          <aside class="usage-panel">
            <div class="usage-header"><span class="usage-symbol">✦</span><div><span class="tool-kicker">WHAT YOU'RE SEEING</span><h3>Read the response path</h3></div></div>
            <div class="usage-step"><span class="step-dot cache-dot"></span><div><strong>Cache</strong><p>Fast repeat requests are served by Redis without touching DBpedia.</p></div></div>
            <div class="usage-step"><span class="step-dot origin-dot"></span><div><strong>Origin</strong><p>A fresh lookup traveled through the bounded outbound gate.</p></div></div>
            <div class="usage-step"><span class="step-dot stale-dot"></span><div><strong>Stale fallback</strong><p>If DBpedia is unavailable, a known response can keep the user moving.</p></div></div>
            <div class="example-block"><span class="tool-kicker">QUICK EXAMPLES</span><div class="example-chips"><button type="button" on:click={() => chooseExample("entity", "Albert_Einstein")}>Albert Einstein</button><button type="button" on:click={() => chooseExample("search", "Ethiopia")}>Ethiopia</button><button type="button" on:click={() => chooseExample("sparql", "SELECT ?s WHERE { ?s a <http://dbpedia.org/ontology/City> } LIMIT 5")}>Cities</button></div></div>
          </aside>
        </div>
      </section>

      <section id="response" class="content-section response-section">
        <div class="section-heading">
          <div>
            <div class="eyebrow">Transparent by design</div>
            <h2>Response inspector</h2>
          </div>
          <div class="response-tools"><span class="response-size">{responseBytes.toLocaleString()} bytes</span><button class="copy-button" type="button" on:click={copyResponse}>{copyState} <span>⧉</span></button></div>
        </div>
        <div class="response-panel">
          <div class="response-bar">
            <div><span class:state-origin={responseSource === "origin"} class:state-cache={responseSource === "cache"} class:state-stale={responseSource === "stale"} class:state-error={responseSource === "error"} class="source-badge">{sourceLabel(responseSource)}</span><strong>{sourceTitle}</strong></div>
            <span class="mono-label">JSON / RAW VIEW</span>
          </div>
          <pre>{responseText}</pre>
        </div>
      </section>
    </main>

    <footer class="workspace-footer">
      <span>KGProxy / DBpedia reliability gateway</span>
      <span>Cache · concurrency · circuit breaker · metrics</span>
    </footer>
  </div>
</div>
