<script lang="ts">
  import { onMount } from "svelte";
  import uPlot from "uplot";
  import "uplot/dist/uPlot.min.css";

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
    { label: "Amharic DBpedia", value: "https://am.dbpedia.data.dice-research.org/sparql" },
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
  let exportState = "Export JSON";

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
  $: trafficPoints = fillTrafficGaps(historyPoints, timeseries.bucket_seconds ?? 3_600);
  $: busiestHour = trafficPoints.length
    ? trafficPoints.reduce((best, point) => point.total_requests > best.total_requests ? point : best, trafficPoints[0])
    : undefined;
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

  let activityChartElement: HTMLDivElement;
  let activityChart: uPlot | undefined;

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

  function fillTrafficGaps(points: MetricsPoint[], bucketSeconds: number) {
    if (!points.length) return [];
    const sorted = [...points].sort((a, b) => a.observed_at_unix_secs - b.observed_at_unix_secs);
    const byBucket = new Map(sorted.map((point) => [point.observed_at_unix_secs, point]));
    const filled: MetricsPoint[] = [];
    for (let timestamp = sorted[0].observed_at_unix_secs; timestamp <= sorted[sorted.length - 1].observed_at_unix_secs; timestamp += bucketSeconds) {
      filled.push(byBucket.get(timestamp) ?? {
        observed_at_unix_secs: timestamp,
        total_requests: 0,
        cache_hit_rate: 0,
        stale_response_rate: 0,
        p95_latency_ms: 0
      });
    }
    return filled;
  }

  function updateActivityChart() {
    if (!activityChartElement || !trafficPoints.length) return;
    const data: uPlot.AlignedData = [
      trafficPoints.map((point) => point.observed_at_unix_secs),
      trafficPoints.map((point) => point.total_requests)
    ];

    if (activityChart) {
      activityChart.setData(data);
      return;
    }

    activityChart = new uPlot(
      {
        width: Math.max(280, activityChartElement.clientWidth),
        height: 250,
        padding: [12, 12, 8, 4],
        scales: {
          x: { time: true },
          y: { range: (_self, min, max) => [0, Math.max(1, max * 1.15)] }
        },
        axes: [
          {
            stroke: "#80908b",
            grid: { stroke: "rgba(16, 33, 43, 0.09)", width: 1 },
            ticks: { stroke: "rgba(16, 33, 43, 0.12)", width: 1 },
            font: "11px IBM Plex Mono, monospace",
            space: 72,
            values: (_self, values) => values.map((value) => formatBucket(Number(value)))
          },
          {
            stroke: "#80908b",
            grid: { stroke: "rgba(16, 33, 43, 0.09)", width: 1 },
            ticks: { stroke: "rgba(16, 33, 43, 0.12)", width: 1 },
            font: "11px IBM Plex Mono, monospace",
            size: 44,
            values: (_self, values) => values.map((value) => number(Number(value)))
          }
        ],
        series: [
          {},
          {
            label: "Questions",
            stroke: "#279e9c",
            width: 2,
            fill: "rgba(103, 214, 207, 0.18)",
            points: { show: true, size: 5, fill: "#279e9c", stroke: "#fbfaf6", width: 2 }
          }
        ],
        cursor: { drag: { x: false, y: false } },
        legend: { show: false }
      },
      data,
      activityChartElement
    );
  }

  onMount(() => {
    const resizeObserver = new ResizeObserver(() => {
      if (activityChart && activityChartElement) {
        activityChart.setSize({ width: Math.max(280, activityChartElement.clientWidth), height: 250 });
      }
    });

    if (activityChartElement) resizeObserver.observe(activityChartElement);
    return () => {
      resizeObserver.disconnect();
      activityChart?.destroy();
    };
  });

  $: if (activityChartElement && trafficPoints.length) updateActivityChart();

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
    exportState = "Export JSON";
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
      if (navigator.clipboard?.writeText) {
        try {
          await navigator.clipboard.writeText(responseText);
        } catch {
          copyWithFallback();
        }
      } else {
        copyWithFallback();
      }
      copyState = "Copied";
      window.setTimeout(() => (copyState = "Copy JSON"), 1600);
    } catch {
      copyState = "Copy unavailable";
    }
  }

  function copyWithFallback() {
    const copyArea = document.createElement("textarea");
    copyArea.value = responseText;
    copyArea.setAttribute("readonly", "");
    copyArea.style.position = "fixed";
    copyArea.style.opacity = "0";
    document.body.appendChild(copyArea);
    copyArea.select();
    const copied = document.execCommand("copy");
    copyArea.remove();
    if (!copied) throw new Error("Copy command was rejected");
  }

  function exportResponse() {
    try {
      const blob = new Blob([responseText], { type: "application/json;charset=utf-8" });
      const downloadUrl = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = downloadUrl;
      link.download = "kgproxy-response.json";
      document.body.appendChild(link);
      link.click();
      link.remove();
      URL.revokeObjectURL(downloadUrl);
      exportState = "Exported";
      window.setTimeout(() => (exportState = "Export JSON"), 1600);
    } catch {
      exportState = "Export unavailable";
    }
  }

  function focusLab() {
    document.getElementById("query-lab")?.scrollIntoView({ behavior: "smooth", block: "start" });
  }

  function chooseLanguage(language: string) {
    entityLanguage = language;
    searchLanguage = language;
    entityEndpoint = "";
    searchEndpoint = "";
    activeTool = "entity";
  }

  refreshDashboard();
</script>

<svelte:head>
  <title>KGProxy / Edge observatory</title>
  <meta
    name="description"
    content="Explore people, places, and ideas from DBpedia in a clear, friendly dashboard."
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
        Recent activity
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
      <p>Ask DBpedia a question and see a clear answer.</p>
      <small>English · አማርኛ · more languages</small>
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
            Look up people, places, and ideas from DBpedia. Try a question below,
            then see where your answer came from.
          </p>
          <div class="language-strip" aria-label="Available DBpedia languages">
            <span class="language-label">Try DBpedia in</span>
            <button class:active={entityLanguage === "en"} type="button" on:click={() => chooseLanguage("en")}>English</button>
            <button class:active={entityLanguage === "am"} type="button" on:click={() => chooseLanguage("am")}>አማርኛ</button>
            <button class:active={entityLanguage === "de"} type="button" on:click={() => chooseLanguage("de")}>Deutsch</button>
            <button class:active={entityLanguage === "fr"} type="button" on:click={() => chooseLanguage("fr")}>Français</button>
          </div>
          <div class="hero-actions">
            <button class="primary-button" type="button" on:click={focusLab}>
              Open query lab
              <span>↗</span>
            </button>
            <a class="text-button" href="#traffic">See recent activity <span>↓</span></a>
          </div>
        </div>

        <div class="route-card" aria-label="KGProxy request path">
          <div class="route-card-top">
            <span class="mono-label">HOW YOUR ANSWER ARRIVES</span>
            <span class="route-time">{lastUpdated}</span>
          </div>
          <div class="route-map">
            <div class="route-line"><span class="route-travel"></span></div>
            <div class="route-node client-node">
              <span class="node-glyph">⌁</span>
              <strong>You ask</strong>
              <small>your question</small>
            </div>
            <div class="route-node cache-node">
              <span class="node-glyph">◌</span>
              <strong>KGProxy</strong>
              <small>finds an answer</small>
            </div>
            <div class="route-node origin-node">
              <span class="node-glyph">✦</span>
              <strong>DBpedia</strong>
              <small>knowledge source</small>
            </div>
          </div>
          <div class="route-caption">
            <span class="caption-dot"></span>
            <span>The path stays visible, so every answer is easy to understand.</span>
          </div>
        </div>
      </section>

      <section id="query-lab" class="content-section query-section">
        <div class="section-heading query-heading">
          <div>
            <div class="eyebrow">Start here</div>
            <h2>Ask DBpedia</h2>
            <p class="section-description query-description">Choose a simple lookup, search for a topic, or write a question. Your answer will appear beside this form.</p>
          </div>
        </div>

        <div class="query-layout">
          <div class="lab-panel">
            <div class="lab-tabs" role="tablist" aria-label="Ways to ask DBpedia">
              <button class:active={activeTool === "entity"} type="button" role="tab" aria-selected={activeTool === "entity"} on:click={() => (activeTool = "entity")}>
                <span>01</span> Look up a thing
              </button>
              <button class:active={activeTool === "search"} type="button" role="tab" aria-selected={activeTool === "search"} on:click={() => (activeTool = "search")}>
                <span>02</span> Search a topic
              </button>
              <button class:active={activeTool === "sparql"} type="button" role="tab" aria-selected={activeTool === "sparql"} on:click={() => (activeTool = "sparql")}>
                <span>03</span> Advanced query
              </button>
            </div>

            {#if activeTool === "entity"}
              <form class="tool-form" on:submit|preventDefault={runEntity}>
                <div class="tool-heading">
                  <div><span class="tool-kicker">LOOK UP A THING</span><h3>Find a person, place, or idea</h3></div>
                  <span class:state-origin={entitySource === "origin"} class:state-cache={entitySource === "cache"} class:state-stale={entitySource === "stale"} class:state-error={entitySource === "error"} class:state-loading={entitySource === "loading"} class="source-badge">{sourceLabel(entitySource)}</span>
                </div>
                <label class="field wide-field"><span>What should we look up?</span><input bind:value={entityId} autocomplete="off" placeholder="Albert_Einstein" /></label>
                <div class="field-row">
                  <label class="field"><span>Knowledge base</span><select bind:value={entityEndpoint}>{#each endpointOptions as option}<option value={option.value}>{option.label}</option>{/each}</select></label>
                  <label class="field"><span>Language</span><select bind:value={entityLanguage}>{#each languageOptions as option}<option value={option.value}>{option.label}</option>{/each}</select></label>
                </div>
                <div class="form-footer"><span class="field-hint">Example: use underscores between words, like Albert_Einstein.</span><button class="run-button" type="submit" disabled={entityLoading}>{entityLoading ? "Finding answer..." : "Find answer"} <span>↗</span></button></div>
              </form>
            {:else if activeTool === "search"}
              <form class="tool-form" on:submit|preventDefault={runSearch}>
                <div class="tool-heading">
                  <div><span class="tool-kicker">SEARCH A TOPIC</span><h3>Explore a subject in DBpedia</h3></div>
                  <span class:state-origin={searchSource === "origin"} class:state-cache={searchSource === "cache"} class:state-stale={searchSource === "stale"} class:state-error={searchSource === "error"} class:state-loading={searchSource === "loading"} class="source-badge">{sourceLabel(searchSource)}</span>
                </div>
                <label class="field wide-field"><span>What are you curious about?</span><input bind:value={searchQuery} autocomplete="off" placeholder="Ethiopia" /></label>
                <div class="field-row">
                  <label class="field"><span>Knowledge base</span><select bind:value={searchEndpoint}>{#each endpointOptions as option}<option value={option.value}>{option.label}</option>{/each}</select></label>
                  <label class="field"><span>Language</span><select bind:value={searchLanguage}>{#each languageOptions as option}<option value={option.value}>{option.label}</option>{/each}</select></label>
                </div>
                <div class="form-footer"><span class="field-hint">Try a country, city, person, or subject.</span><button class="run-button" type="submit" disabled={searchLoading}>{searchLoading ? "Searching..." : "Search"} <span>↗</span></button></div>
              </form>
            {:else}
              <form class="tool-form" on:submit|preventDefault={runSparql}>
                <div class="tool-heading">
                  <div><span class="tool-kicker">ADVANCED QUERY</span><h3>Use a custom DBpedia question</h3></div>
                  <span class:state-origin={sparqlSource === "origin"} class:state-cache={sparqlSource === "cache"} class:state-stale={sparqlSource === "stale"} class:state-error={sparqlSource === "error"} class:state-loading={sparqlSource === "loading"} class="source-badge">{sourceLabel(sparqlSource)}</span>
                </div>
                <label class="field wide-field"><span>Knowledge base</span><select bind:value={sparqlEndpoint}>{#each endpointOptions as option}<option value={option.value}>{option.label}</option>{/each}</select></label>
                <label class="field wide-field"><span>Your question</span><textarea bind:value={sparqlQuery} spellcheck="false" placeholder="SELECT * WHERE &#123; ?s ?p ?o &#125; LIMIT 5"></textarea></label>
                <div class="form-footer"><span class="field-hint">For users familiar with DBpedia's query language.</span><button class="run-button" type="submit" disabled={sparqlLoading}>{sparqlLoading ? "Running..." : "Run query"} <span>↗</span></button></div>
              </form>
            {/if}
          </div>

          <div class="response-column">
            <section id="response" class="response-card">
              <div class="response-heading">
                <div><div class="eyebrow">Your result</div><h2>Answer</h2></div>
                <div class="response-actions">
                  <button class="copy-button" type="button" on:click={copyResponse}>{copyState} <span>⧉</span></button>
                  <button class="export-button" type="button" on:click={exportResponse}>{exportState} <span>↓</span></button>
                </div>
              </div>
              <div class="response-panel">
                <div class="response-bar">
                  <div><span class:state-origin={responseSource === "origin"} class:state-cache={responseSource === "cache"} class:state-stale={responseSource === "stale"} class:state-error={responseSource === "error"} class="source-badge">{sourceLabel(responseSource)}</span><strong>{sourceTitle}</strong></div>
                  <span class="response-size">{responseBytes.toLocaleString()} bytes</span>
                </div>
                <pre>{responseText}</pre>
              </div>
              <p class="response-help">The label above tells you how this answer was found. “Redis cache” means KGProxy already had the answer ready; “DBpedia origin” means it looked it up just now.</p>
            </section>
            <aside class="usage-panel plain-usage">
              <div class="usage-header"><span class="usage-symbol">✦</span><div><span class="tool-kicker">QUICK START</span><h3>Try an example</h3></div></div>
              <p class="usage-intro">Start with one of these examples, then change the words to explore.</p>
              <div class="example-chips"><button type="button" on:click={() => chooseExample("entity", "Albert_Einstein")}>Albert Einstein</button><button type="button" on:click={() => chooseExample("search", "Ethiopia")}>Ethiopia</button><button type="button" on:click={() => chooseExample("sparql", "SELECT ?s WHERE { ?s a <http://dbpedia.org/ontology/City> } LIMIT 5")}>Cities</button></div>
            </aside>
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
          <div class="metric-top"><span class="metric-label">Questions today</span><span class="metric-index">02</span></div>
          <div class="metric-value">{number(metrics.total_requests)}</div>
          <div class="metric-foot"><span>latest hour</span><strong>{trafficPoints.length ? number(trafficPoints[trafficPoints.length - 1].total_requests) : "—"}</strong></div>
        </article>
        <article class="metric-card metric-cache">
          <div class="metric-top"><span class="metric-label">Fast repeat answers</span><span class="metric-index">03</span></div>
          <div class="metric-value">{percent(metrics.cache_hit_rate)}</div>
          <div class="metric-foot"><span>previously answered</span><strong>{number(metrics.cache_hits)}</strong></div>
        </article>
        <article class="metric-card metric-latency">
          <div class="metric-top"><span class="metric-label">Typical response</span><span class="metric-index">04</span></div>
          <div class="metric-value">{number(metrics.p95_latency_ms)}<small>ms</small></div>
          <div class="metric-foot"><span>last 24 hours</span><strong>{number(metrics.origin_errors)} issues</strong></div>
        </article>
      </section>

      <section id="traffic" class="content-section">
        <div class="activity-header">
          <div class="activity-title">
            <div class="eyebrow">What people asked recently</div>
            <h2>Recent activity</h2>
            <p class="section-description traffic-description">Each point represents one hour. Quiet hours stay visible, so the shape reflects the full day.</p>
          </div>
          <div class="activity-summary" aria-label="Recent activity summary">
            <div class="activity-stat">
              <strong>{number(metrics.total_requests)}</strong>
              <span>questions in 24 hours</span>
            </div>
            <div class="activity-stat">
              <strong>{number(busiestHour?.total_requests)}</strong>
              <span>busiest hour</span>
            </div>
          </div>
        </div>

        <div class="traffic-panel">
          {#if trafficPoints.length}
            <div class="chart-label-row">
              <span class="legend"><i class="legend-line"></i> questions per hour</span>
              <span class="mono-label">LAST 24 HOURS</span>
            </div>
            <div class="chart-wrap">
              <div class="activity-chart" bind:this={activityChartElement} role="img" aria-label="Questions asked each hour"></div>
            </div>
            <div class="chart-footer">
              <span>{formatBucket(trafficPoints[0].observed_at_unix_secs)}</span>
              <strong>{number(busiestHour?.total_requests)} questions at {busiestHour ? formatBucket(busiestHour.observed_at_unix_secs) : "—"}</strong>
              <span>{formatBucket(trafficPoints[trafficPoints.length - 1].observed_at_unix_secs)}</span>
            </div>
          {:else}
            <div class="empty-state">
              <span class="empty-glyph">⌁</span>
              <strong>No recent activity yet</strong>
              <span>Ask your first question above and it will appear here.</span>
            </div>
          {/if}
        </div>
      </section>

    </main>

    <footer class="workspace-footer">
      <span>KGProxy / a clearer way to explore DBpedia</span>
      <span>English · አማርኛ · Deutsch · Français</span>
    </footer>
  </div>
</div>
