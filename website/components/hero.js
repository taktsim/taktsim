// The welcome page's logo animation (design system, Motion): starts the
// playhead when the logo is actually on screen, and again on hover, click,
// tap or Enter. The animation itself is CSS (website/theme/taktsim.css);
// this only sets the class ts-play at the right moments.
(function () {
  "use strict";
  const PLAY_MS = 3942; // a quarter-second pause and two bars at 130 BPM

  function setup(svg) {
    const still = window.matchMedia("(prefers-reduced-motion: reduce)");
    let playing = false;
    let played = false;

    function play() {
      if (playing || still.matches) return;
      playing = true;
      played = true;
      svg.classList.remove("ts-play");
      void svg.getBoundingClientRect(); // restart the CSS animation
      svg.classList.add("ts-play");
      setTimeout(() => { playing = false; }, PLAY_MS);
    }

    // Start once the logo is visible in a visible tab.
    let inView = false;
    const maybeStart = () => {
      if (!played && inView && document.visibilityState === "visible") play();
    };
    if ("IntersectionObserver" in window) {
      new IntersectionObserver((entries) => {
        inView = entries.some((e) => e.isIntersecting);
        maybeStart();
      }, { threshold: 0.6 }).observe(svg);
    } else {
      inView = true;
    }
    document.addEventListener("visibilitychange", maybeStart);
    maybeStart();

    // Replay on demand.
    svg.classList.add("ts-interactive");
    svg.setAttribute("tabindex", "0");
    svg.addEventListener("mouseenter", play);
    svg.addEventListener("click", play);
    svg.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        play();
      }
    });
  }

  const start = () => document.querySelectorAll("svg.ts-hero").forEach(setup);
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start);
  else start();
})();
