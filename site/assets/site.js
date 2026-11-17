// Highlights the download for the visitor's system, and the guide section
// being read in the table of contents.
(() => {
  const ua = navigator.userAgent;
  const os = /Windows/.test(ua) ? "windows" : /Mac OS X|Macintosh/.test(ua) ? "macos" : /Linux|X11/.test(ua) ? "linux" : null;
  if (os) {
    document.querySelectorAll(`[data-os="${os}"]`).forEach((el) => el.classList.add("here"));
    const main = document.querySelector("[data-download-main]");
    const label = main && main.dataset[os];
    if (label) main.textContent = label;
  }

  const links = [...document.querySelectorAll(".toc a")];
  if (!links.length || !("IntersectionObserver" in window)) return;
  const byId = new Map(links.map((a) => [a.getAttribute("href").slice(1), a]));
  const observer = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      links.forEach((a) => a.classList.remove("active"));
      byId.get(entry.target.id)?.classList.add("active");
    }
  }, { rootMargin: "-20% 0px -70% 0px" });
  byId.forEach((_, id) => { const h = document.getElementById(id); if (h) observer.observe(h); });
})();
