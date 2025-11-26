(() => {
  const searchInput = document.querySelector('[type="search"]');
  const resultsList = document.querySelector('ul[aria-label="Search Results"]');
  const { searchIndex, Fuse } = window;

  if (!searchInput || !resultsList || !searchIndex || !searchIndex.length || !Fuse) {
    return;
  }

  // const documents = Object.values(searchIndex.documentStore.docs || {}).map((doc) => ({
  //   id: doc.id,
  //   title: doc.title || doc.id,
  //   body: doc.body || "",
  // }));

  const fuse = new Fuse(searchIndex, {
    keys: [
      { name: "title", weight: 2 },
      { name: "body", weight: 1 },
    ],
    threshold: 0.35,
    ignoreLocation: true,
    minMatchCharLength: 2,
  });

  const searchForm = searchInput.closest("form");
  if (searchForm) {
    searchForm.addEventListener("submit", (event) => event.preventDefault());
  }

  const MIN_QUERY_LENGTH = 2;
  const RESULT_LIMIT = 8;

  let lastQuery = "";

  const normalize = (text) => (text || "").replace(/\s+/g, " ").trim();

  const buildSnippet = (text, query) => {
    const normalized = normalize(text);
    if (!normalized) {
      return "";
    }

    const lower = normalized.toLowerCase();
    const needle = query.toLowerCase();
    const index = lower.indexOf(needle);
    const SNIPPET_PADDING = 70;

    if (index === -1) {
      const snippet = normalized.slice(0, 140);
      return normalized.length > 140 ? `${snippet}...` : snippet;
    }

    const start = Math.max(0, index - SNIPPET_PADDING);
    const end = Math.min(normalized.length, index + needle.length + SNIPPET_PADDING);

    let snippet = normalized.slice(start, end).trim();
    if (start > 0) {
      snippet = `...${snippet}`;
    }
    if (end < normalized.length) {
      snippet = `${snippet}...`;
    }

    return snippet;
  };

  const setBusy = (isBusy) => {
    if (isBusy) {
      resultsList.setAttribute("aria-busy", "true");
    } else {
      resultsList.removeAttribute("aria-busy");
    }
  };

  const hideResults = () => {
    resultsList.innerHTML = "";
    resultsList.hidden = true;
    setBusy(false);
  };

  const renderResults = (matches, query) => {
    resultsList.innerHTML = "";

    if (!matches.length) {
      const li = document.createElement("li");
      li.textContent = `No results for "${query}"`;
      resultsList.appendChild(li);
      return;
    }

    matches.forEach(({ item }) => {
      const li = document.createElement("li");
      const link = document.createElement("a");
      link.href = item.url;
      link.textContent = item.title;

      const summary = document.createElement("summary");
      summary.textContent = buildSnippet(item.body, query);

      li.appendChild(link);
      if (summary.textContent) {
        li.appendChild(summary);
      }
      resultsList.appendChild(li);
    });
  };

  const showResults = () => {
    if (resultsList.hidden) {
      resultsList.hidden = false;
    }
  };

  const performSearch = (value) => {
    const query = value.trim();
    lastQuery = query;

    if (query.length < MIN_QUERY_LENGTH) {
      hideResults();
      return;
    }

    showResults();
    setBusy(true);

    window.requestAnimationFrame(() => {
      const matches = fuse.search(query, { limit: RESULT_LIMIT });
      renderResults(matches, query);
      setBusy(false);
    });
  };

  const debounce = (fn, delay = 150) => {
    let timeout;
    return (...args) => {
      window.clearTimeout(timeout);
      timeout = window.setTimeout(() => fn(...args), delay);
    };
  };

  const debouncedSearch = debounce(performSearch, 150);

  searchInput.addEventListener("input", (event) => {
    debouncedSearch(event.target.value);
  });

  searchInput.addEventListener("focus", () => {
    if (lastQuery.length >= MIN_QUERY_LENGTH && resultsList.children.length) {
      showResults();
    }
  });

  searchInput.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      searchInput.value = "";
      lastQuery = "";
      hideResults();
    }
  });

  document.addEventListener("click", (event) => {
    const path = event.composedPath ? event.composedPath() : [event.target];
    const clickedInsideSearch =
      (searchForm && path.includes(searchForm)) || path.includes(resultsList);

    if (!clickedInsideSearch) {
      hideResults();
    }
  });

  // Expose the input once the search index is wired up.
  searchInput.hidden = false;
})();
