"use strict";

(() => {
  const byId = (id) => document.getElementById(id);
  const gallery = byId("gallery");
  const search = byId("search");
  const notice = byId("notice");
  const dialog = byId("comparison-dialog");
  const filters = Array.from(document.querySelectorAll("[data-filter]"));
  let cases = [];
  let visible = [];
  let activeFilter = "all";
  let activeIndex = -1;
  let openingButton = null;
  let showDiff = false;
  let reportSettings = {};
  let imageVersion = "";

  function element(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  }

  function text(value, fallback = "") {
    return typeof value === "string" ? value : fallback;
  }

  function kindLabel(kind) {
    return kind === "native-geometry" ? "Fresh geometry" : kind === "saved-preview" ? "Saved preview" : kind === "image" ? "Image" : "Model";
  }

  function assetUrl(value, external = false) {
    if (typeof value !== "string" || !value.trim()) return null;
    try {
      const url = new URL(value, document.baseURI);
      if (!["http:", "https:"].includes(url.protocol) || (!external && url.origin !== location.origin)) return null;
      if (!external && imageVersion) url.searchParams.set("v", imageVersion);
      return url.href;
    } catch (_) {
      return null;
    }
  }

  function statusInfo(status) {
    const key = text(status).toLowerCase();
    if (["match", "matched", "pass", "passed", "ok", "unchanged"].includes(key)) return { label: "Match", className: "status-match" };
    if (["changed", "mismatch", "different", "fail", "failed"].includes(key)) return { label: "Changed", className: "status-changed" };
    if (["error", "render-error", "missing", "missing-baseline", "missing-actual"].includes(key)) return { label: "Unavailable", className: "status-error" };
    return { label: key ? text(status) : "Unverified", className: "" };
  }

  function badge(status) {
    const info = statusInfo(status);
    return element("span", `status-badge ${info.className}`, info.label);
  }

  function link(href, label, className) {
    const url = assetUrl(href, true);
    if (!url) return null;
    const node = element("a", className, label);
    node.href = url;
    if (new URL(url).origin !== location.origin) {
      node.target = "_blank";
      node.rel = "noopener noreferrer";
    }
    return node;
  }

  function card(item, index) {
    const article = element("article", "gallery-card");
    const preview = element("button", "card-preview checkerboard");
    preview.type = "button";
    preview.setAttribute("aria-label", `Compare ${text(item.title, item.id)} (${text(item.extension)})`);
    const actual = assetUrl(item.actual_url);
    const placeholder = element("span", "image-placeholder", item.kind === "saved-preview" ? "Saved preview unavailable" : "Current output unavailable");
    if (actual) {
      const image = element("img");
      image.alt = text(item.title, "Preview thumbnail");
      image.loading = "lazy";
      image.decoding = "async";
      image.addEventListener("error", () => image.replaceWith(placeholder), { once: true });
      image.src = actual;
      preview.append(image);
    } else {
      preview.append(placeholder);
    }
    preview.addEventListener("click", () => openComparison(index, preview));
    const body = element("div", "card-body");
    const heading = element("div", "card-heading");
    heading.append(element("span", "format-badge", `.${text(item.extension, "?")}`), badge(item.status));
    const title = element("h2");
    const titleButton = element("button", "card-title", text(item.title, item.id));
    titleButton.type = "button";
    titleButton.addEventListener("click", () => openComparison(index, titleButton));
    title.append(titleButton);
    const footer = element("div", "card-footer");
    footer.append(element("span", "", kindLabel(item.kind)));
    const source = link(item.input_url, "Source ↗");
    if (source) footer.append(source);
    body.append(heading, title, footer);
    article.append(preview, body);
    return article;
  }

  function renderCollection() {
    const query = search.value.trim().toLowerCase();
    visible = cases.filter((item) => (activeFilter === "all" || item.kind === activeFilter) && `${text(item.extension)} ${text(item.title)} ${text(item.id)}`.toLowerCase().includes(query));
    const fragment = document.createDocumentFragment();
    visible.forEach((item, index) => fragment.append(card(item, index)));
    gallery.replaceChildren(fragment);
    byId("results-count").textContent = `${visible.length.toLocaleString()} of ${cases.length.toLocaleString()} examples`;
    notice.hidden = visible.length > 0;
    notice.className = "notice";
    notice.textContent = "No examples match your search. Try a file extension or another title.";
  }

  function number(value, digits = 0) {
    return Number.isFinite(value) ? value.toLocaleString(undefined, { maximumFractionDigits: digits }) : "—";
  }

  function setImage(id, url, alt, unavailable) {
    const image = byId(id);
    const placeholder = image.parentElement.querySelector(".image-placeholder");
    const href = assetUrl(url);
    image.onload = () => { image.hidden = false; placeholder.hidden = true; };
    image.onerror = () => { image.hidden = true; placeholder.hidden = false; };
    image.alt = alt;
    placeholder.textContent = unavailable;
    image.hidden = true;
    placeholder.hidden = false;
    if (href) image.src = href;
    else image.removeAttribute("src");
  }

  function download(id, value, filename, enabled = true) {
    const node = byId(id);
    const href = enabled ? assetUrl(value, id === "comparison-source") : null;
    node.hidden = !href;
    if (href) {
      node.href = href;
      if (filename) node.download = filename;
      else node.removeAttribute("download");
      if (id === "comparison-source" && new URL(href).origin !== location.origin) {
        node.target = "_blank";
        node.rel = "noopener noreferrer";
      } else {
        node.removeAttribute("target");
        node.removeAttribute("rel");
      }
    } else {
      node.removeAttribute("href");
      node.removeAttribute("download");
    }
  }

  function updateWipe() {
    const value = Number(byId("wipe-slider").value);
    byId("current-layer").style.clipPath = `inset(0 ${100 - value}% 0 0)`;
    byId("wipe-divider").style.left = `${value}%`;
    byId("wipe-slider").setAttribute("aria-valuetext", `${value} percent current output`);
  }

  function updateMode() {
    byId("diff-toggle").setAttribute("aria-pressed", String(showDiff));
    byId("diff-toggle").textContent = showDiff ? "Show comparison" : "Show diff";
    byId("diff-layer").hidden = !showDiff;
    byId("baseline-layer").hidden = showDiff;
    byId("current-layer").hidden = showDiff;
    byId("wipe-divider").hidden = showDiff;
    byId("comparison-labels").hidden = showDiff;
    byId("wipe-controls").hidden = showDiff;
  }

  function populateComparison() {
    const item = visible[activeIndex];
    if (!item) return;
    const title = text(item.title, item.id);
    const info = statusInfo(item.status);
    byId("comparison-title").textContent = title;
    byId("comparison-extension").textContent = `.${text(item.extension)} · ${kindLabel(item.kind)}`;
    byId("comparison-carrier-note").hidden = item.kind !== "saved-preview";
    dialog.setAttribute("aria-describedby", item.kind === "saved-preview" ? "comparison-description comparison-carrier-note" : "comparison-description");
    const renderError = text(item.error);
    byId("comparison-description").textContent = renderError
      ? `${info.className === "status-changed" ? "Snapshot comparison changed" : "Snapshot unavailable"}: ${renderError}`
      : `Current output compared with the approved PNG baseline. Status: ${info.label.toLowerCase()}.`;
    byId("comparison-status").textContent = info.label;
    byId("comparison-status").className = `status-badge ${info.className}`;
    byId("comparison-position").textContent = `${activeIndex + 1} / ${visible.length}`;
    byId("previous-case").disabled = activeIndex <= 0;
    byId("next-case").disabled = activeIndex >= visible.length - 1;
    setImage("baseline-image", item.expected_url, `${title}, approved baseline`, "Baseline snapshot unavailable");
    setImage("current-image", item.actual_url, `${title}, current output`, item.kind === "saved-preview" ? "Saved preview unavailable" : "Current output unavailable");
    setImage("diff-image", item.diff_url, `${title}, pixel difference`, "Pixel difference image unavailable");
    byId("diff-toggle").disabled = !assetUrl(item.diff_url);
    const metric = item.metrics && typeof item.metrics === "object" ? item.metrics : {};
    byId("max-delta").textContent = number(metric.max_delta);
    byId("changed-pixels").textContent = number(metric.changed_pixels);
    byId("mean-delta").textContent = number(metric.mean_delta, 4);
    byId("comparison-provenance").textContent = text(item.provenance, "Provenance was not included in this report.");
    const filename = text(item.id, "thumbnail").replace(/[^a-zA-Z0-9_-]/g, "-");
    download("current-download", item.actual_url, `${filename}-current.png`);
    download("baseline-download", item.expected_url, `${filename}-baseline.png`);
    download("diff-download", item.diff_url, `${filename}-diff.png`);
    download("comparison-source", item.input_url);
    if (Number.isFinite(reportSettings.width) && Number.isFinite(reportSettings.height) && reportSettings.width > 0 && reportSettings.height > 0) {
      byId("comparison-stage").style.aspectRatio = `${reportSettings.width} / ${reportSettings.height}`;
    }
    showDiff = false;
    byId("wipe-slider").value = "50";
    updateWipe();
    updateMode();
  }

  function openComparison(index, trigger) {
    activeIndex = index;
    openingButton = trigger;
    populateComparison();
    dialog.showModal();
  }

  function moveComparison(delta) {
    const index = activeIndex + delta;
    if (index >= 0 && index < visible.length) {
      activeIndex = index;
      populateComparison();
    }
  }

  filters.forEach((button) => button.addEventListener("click", () => {
    activeFilter = button.dataset.filter;
    filters.forEach((filter) => filter.setAttribute("aria-pressed", String(filter === button)));
    renderCollection();
  }));
  search.addEventListener("input", renderCollection);
  byId("close-dialog").addEventListener("click", () => dialog.close());
  dialog.addEventListener("close", () => { if (openingButton && openingButton.isConnected) openingButton.focus(); });
  dialog.addEventListener("click", (event) => {
    if (event.target !== dialog) return;
    const box = dialog.getBoundingClientRect();
    if (event.clientX < box.left || event.clientX > box.right || event.clientY < box.top || event.clientY > box.bottom) dialog.close();
  });
  dialog.addEventListener("keydown", (event) => {
    if (event.target instanceof HTMLInputElement || event.ctrlKey || event.metaKey || event.altKey) return;
    if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      event.preventDefault();
      moveComparison(event.key === "ArrowLeft" ? -1 : 1);
    }
  });
  byId("previous-case").addEventListener("click", () => moveComparison(-1));
  byId("next-case").addEventListener("click", () => moveComparison(1));
  byId("wipe-slider").addEventListener("input", updateWipe);
  byId("diff-toggle").addEventListener("click", () => { showDiff = !showDiff; updateMode(); });

  async function loadReport() {
    try {
      const response = await fetch("./data.json", { cache: "no-cache" });
      if (!response.ok) throw new Error(`The report could not be loaded (HTTP ${response.status}).`);
      const report = await response.json();
      if (!report || report.schema !== 1 || !Array.isArray(report.cases)) throw new Error("The gallery report has an unsupported structure.");
      cases = report.cases.filter((item) => item && typeof item === "object");
      reportSettings = report.settings && typeof report.settings === "object" ? report.settings : {};
      imageVersion = text(report.artifact_images_sha256, text(report.commit));
      const extensionCount = Number.isFinite(report.extensionCount) ? report.extensionCount : new Set(cases.map((item) => item.extension)).size;
      byId("extension-count").textContent = number(extensionCount);
      byId("case-count").textContent = number(cases.length);
      byId("matching-count").textContent = number(cases.filter((item) => statusInfo(item.status).className === "status-match").length);
      const settings = [];
      if (Number.isFinite(reportSettings.width) && Number.isFinite(reportSettings.height)) settings.push(`${reportSettings.width} × ${reportSettings.height} PNG`);
      if (reportSettings.transparent) settings.push("transparent");
      if (Number.isFinite(reportSettings.samples)) settings.push(`${reportSettings.samples}× supersampling`);
      byId("render-settings").textContent = settings.join(" · ");
      const details = [];
      if (text(report.renderedAt)) {
        const date = new Date(report.renderedAt);
        if (!Number.isNaN(date.getTime())) details.push(`Built ${date.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" })}`);
      }
      if (text(report.commit)) details.push(`Commit ${report.commit.slice(0, 8)}`);
      byId("build-details").textContent = details.join(" · ");
      search.disabled = false;
      filters.forEach((button) => { button.disabled = false; });
      renderCollection();
    } catch (error) {
      byId("results-count").textContent = "Gallery unavailable";
      notice.className = "notice notice-error";
      notice.textContent = error instanceof Error ? error.message : "The gallery report could not be loaded.";
      notice.hidden = false;
    }
  }

  loadReport();
})();
