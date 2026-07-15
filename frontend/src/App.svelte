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
  let responsePayload: unknown = { status: "ready" };
  let lastUpdated = "not refreshed";
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

  $: serviceStatus = health.status ?? "waiting";
  $: breakerLabel = health.circuit_breaker_state
    ? `breaker ${health.circuit_breaker_state}`
    : "breaker unknown";
  $: permitsLabel =
    health.outbound_available_permits !== undefined &&
    health.max_outbound_concurrency !== undefined
      ? `${health.outbound_available_permits}/${health.max_outbound_concurrency} permits`
      : "permits unknown";
  $: historyPoints = timeseries.points ?? [];
  $: historyPolyline = chartPolyline(historyPoints);
  $: historyArea = `${historyPolyline} 100,100 0,100`;

  function apiUrl(path: string) {
    const base = apiBase.trim().replace(/\/$/, "");
    return `${base}${path}`;
  }

  function withEndpoint(path: string, endpoint: string, language?: string) {
    const params = new URLSearchParams();
    if (endpoint) params.set("endpoint", endpoint);
    if (language) params.set("lang", language);
    const query = params.toString();
    if (!query) return path;
    return `${path}${path.includes("?") ? "&" : "?"}${query}`;
  }

  function percent(value: number | undefined) {
    if (!Number.isFinite(value)) return "0%";
    return `${Math.round((value ?? 0) * 100)}%`;
  }

  function pretty(payload: unknown) {
    return JSON.stringify(payload, null, 2);
  }

  function chartPolyline(points: MetricsPoint[]) {
    if (!points.length) return "";
    return points
      .map((point, index) => {
        const x = points.length === 1 ? 50 : (index / (points.length - 1)) * 100;
        const y = 100 - (point.total_requests / Math.max(1, ...points.map((item) => item.total_requests))) * 84 - 8;
        return `${x.toFixed(2)},${y.toFixed(2)}`;
      })
      .join(" ");
  }

  function formatBucket(unixSeconds: number) {
    return new Date(unixSeconds * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  }

  function updateResponse(payload: unknown) {
    responsePayload = payload;
    lastUpdated = new Date().toLocaleTimeString();
  }

  function sourceFrom(payload: ApiEnvelope) {
    if (payload.error) return "error";
    if (payload.stale) return "stale";
    return payload.source ?? "origin";
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
      const path = withEndpoint(`/v1/entity/${encodeURIComponent(entityId.trim())}`, entityEndpoint, entityLanguage);
      const payload = await requestJson<ApiEnvelope>(path);
      entitySource = sourceFrom(payload);
      updateResponse(payload);
      await refreshDashboard();
    } catch (error) {
      const payload = ((error as { payload?: ApiEnvelope }).payload ?? {
        error: { message: String(error) }
      }) as ApiEnvelope;
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
      const path = withEndpoint(`/v1/search?q=${encodeURIComponent(searchQuery.trim())}`, searchEndpoint, searchLanguage);
      const payload = await requestJson<ApiEnvelope>(path);
      searchSource = sourceFrom(payload);
      updateResponse(payload);
      await refreshDashboard();
    } catch (error) {
      const payload = ((error as { payload?: ApiEnvelope }).payload ?? {
        error: { message: String(error) }
      }) as ApiEnvelope;
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
    const body: { query: string; endpoint?: string } = { query: sparqlQuery.trim() };
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
      const payload = ((error as { payload?: ApiEnvelope }).payload ?? {
        error: { message: String(error) }
      }) as ApiEnvelope;
      sparqlSource = "error";
      updateResponse(payload);
    } finally {
      sparqlLoading = false;
    }
  }

  refreshDashboard();
</script>

<main class="mx-auto w-[min(1180px,calc(100%-32px))] py-7 sm:w-[min(1180px,calc(100%-20px))]">
  <section class="mb-5 grid gap-5 md:flex md:items-end md:justify-between">
    <div>
      <p class="eyebrow mb-2">KGProxy / operations</p>
      <h1 class="text-3xl font-extrabold tracking-normal text-ink sm:text-2xl">
        Reliability Dashboard
      </h1>
      <p class="mt-2 max-w-xl text-sm leading-relaxed text-muted">
        Watch the gateway, test DBpedia language routes, and inspect the response path in one place.
      </p>
    </div>

    <div class="grid gap-3 sm:flex sm:items-end">
      <label class="min-w-0 sm:min-w-80">
        <span class="field-label">API Base</span>
        <input class="field-control" bind:value={apiBase} placeholder="same origin" />
      </label>
      <button class="action-button" type="button" on:click={refreshDashboard} disabled={loadingDashboard}>
        {loadingDashboard ? "Refreshing…" : "Refresh data"}
      </button>
    </div>
  </section>

  <section class="panel mb-4 overflow-hidden p-4 sm:p-5">
    <div class="mb-4 flex flex-wrap items-end justify-between gap-3">
      <div>
        <p class="eyebrow">Traffic pulse</p>
        <h2 class="mt-1 text-lg font-extrabold">Requests by hour</h2>
      </div>
      <span class="text-xs font-bold uppercase tracking-wide text-muted">last 24 hours</span>
    </div>
    {#if historyPoints.length}
      <div class="chart-shell" aria-label="Requests by hour chart">
        <svg viewBox="0 0 100 100" preserveAspectRatio="none" role="img" aria-label="Request volume over time">
          <line class="chart-grid" x1="0" y1="8" x2="100" y2="8" />
          <line class="chart-grid" x1="0" y1="50" x2="100" y2="50" />
          <line class="chart-grid" x1="0" y1="92" x2="100" y2="92" />
          <polygon class="chart-area" points={historyArea} />
          <polyline class="chart-line" points={historyPolyline} />
        </svg>
      </div>
      <div class="mt-3 flex justify-between text-xs font-semibold text-muted">
        <span>{formatBucket(historyPoints[0].observed_at_unix_secs)}</span>
        <span>{historyPoints[historyPoints.length - 1].total_requests} requests in latest bucket</span>
        <span>{formatBucket(historyPoints[historyPoints.length - 1].observed_at_unix_secs)}</span>
      </div>
    {:else}
      <div class="empty-state">
        <strong>No traffic history yet.</strong>
        <span>Run a lookup or search to seed the hourly view.</span>
      </div>
    {/if}
  </section>

  <section class="mb-4 grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
    <article class="panel min-h-32 p-4">
      <span class="field-label">Service</span>
      <strong class="mt-2 block text-3xl">{serviceStatus}</strong>
      <small class="text-muted">{breakerLabel}</small>
    </article>
    <article class="panel min-h-32 p-4">
      <span class="field-label">Requests</span>
      <strong class="mt-2 block text-3xl">{metrics.total_requests ?? 0}</strong>
      <small class="text-muted">last 24h</small>
    </article>
    <article class="panel min-h-32 p-4">
      <span class="field-label">Cache Hit Rate</span>
      <strong class="mt-2 block text-3xl">{percent(metrics.cache_hit_rate)}</strong>
      <small class="text-muted">{metrics.cache_hits ?? 0} hits, {metrics.stale_responses ?? 0} stale</small>
    </article>
    <article class="panel min-h-32 p-4">
      <span class="field-label">P95 Latency</span>
      <strong class="mt-2 block text-3xl">{metrics.p95_latency_ms ?? 0} ms</strong>
      <small class="text-muted">{metrics.origin_errors ?? 0} origin errors, {permitsLabel}</small>
    </article>
  </section>

  <section class="grid gap-4 lg:grid-cols-[0.92fr_1.08fr]">
    <div class="grid gap-4">
      <form class="panel grid gap-4 p-4" on:submit|preventDefault={runEntity}>
        <div class="flex items-center justify-between gap-3">
          <h2 class="text-base font-extrabold">Entity Lookup</h2>
          <span class={`badge ${entitySource}`}>{entitySource}</span>
        </div>
        <div class="grid gap-3 sm:grid-cols-[1fr_180px_150px]">
          <label>
            <span class="field-label">Entity ID</span>
            <input class="field-control" bind:value={entityId} autocomplete="off" />
          </label>
          <label>
            <span class="field-label">Endpoint</span>
            <select class="field-control" bind:value={entityEndpoint}>
              {#each endpointOptions as option}
                <option value={option.value}>{option.label}</option>
              {/each}
            </select>
          </label>
          <label>
            <span class="field-label">Language</span>
            <select class="field-control" bind:value={entityLanguage}>
              {#each languageOptions as option}
                <option value={option.value}>{option.label}</option>
              {/each}
            </select>
          </label>
        </div>
        <button class="action-button" type="submit" disabled={entityLoading}>Run Lookup</button>
      </form>

      <form class="panel grid gap-4 p-4" on:submit|preventDefault={runSearch}>
        <div class="flex items-center justify-between gap-3">
          <h2 class="text-base font-extrabold">Entity Search</h2>
          <span class={`badge ${searchSource}`}>{searchSource}</span>
        </div>
        <div class="grid gap-3 sm:grid-cols-[1fr_180px_150px]">
          <label>
            <span class="field-label">Search Query</span>
            <input class="field-control" bind:value={searchQuery} autocomplete="off" />
          </label>
          <label>
            <span class="field-label">Endpoint</span>
            <select class="field-control" bind:value={searchEndpoint}>
              {#each endpointOptions as option}
                <option value={option.value}>{option.label}</option>
              {/each}
            </select>
          </label>
          <label>
            <span class="field-label">Language</span>
            <select class="field-control" bind:value={searchLanguage}>
              {#each languageOptions as option}
                <option value={option.value}>{option.label}</option>
              {/each}
            </select>
          </label>
        </div>
        <button class="action-button" type="submit" disabled={searchLoading}>Run Search</button>
      </form>
    </div>

    <form class="panel grid gap-4 p-4" on:submit|preventDefault={runSparql}>
      <div class="flex items-center justify-between gap-3">
        <h2 class="text-base font-extrabold">Raw SPARQL</h2>
        <span class={`badge ${sparqlSource}`}>{sparqlSource}</span>
      </div>
      <label>
        <span class="field-label">Endpoint</span>
        <select class="field-control" bind:value={sparqlEndpoint}>
          {#each endpointOptions as option}
            <option value={option.value}>{option.label}</option>
          {/each}
        </select>
      </label>
      <label>
        <span class="field-label">Query</span>
        <textarea
          class="field-control min-h-48 font-mono leading-relaxed"
          bind:value={sparqlQuery}
        ></textarea>
      </label>
      <button class="action-button" type="submit" disabled={sparqlLoading}>Run Query</button>
    </form>
  </section>

  <section class="panel mt-4 p-4">
    <div class="flex items-center justify-between gap-3">
      <h2 class="text-base font-extrabold">Response</h2>
      <span class="text-sm text-muted">{lastUpdated}</span>
    </div>
    <pre class="mt-4 max-h-[520px] min-h-72 overflow-auto rounded-md bg-slate-950 p-4 text-sm leading-relaxed text-slate-100">{pretty(responsePayload)}</pre>
  </section>
</main>
