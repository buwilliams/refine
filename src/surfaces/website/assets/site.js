(function () {
  const origin = window.location.origin;
  const CHECK_ICON = '<svg aria-hidden="true" viewBox="0 0 24 24"><path d="M20 6 9 17l-5-5"></path></svg>';
  const copyButtonState = new WeakMap();

  function absoluteUrl(path) {
    return new URL(path, origin).toString();
  }

  function fillOriginText() {
    document.querySelectorAll("[data-origin-template]").forEach((element) => {
      element.textContent = element.dataset.originTemplate.replaceAll("{origin}", origin);
    });
    document.querySelectorAll("[data-origin-path]").forEach((element) => {
      element.textContent = absoluteUrl(element.dataset.originPath);
    });
  }

  async function copyText(text, button) {
    await navigator.clipboard.writeText(text);
    const state = copyButtonState.get(button) || {
      label: button.getAttribute("aria-label") || "Copy",
      html: button.innerHTML,
      timeout: null,
    };
    if (state.timeout) {
      window.clearTimeout(state.timeout);
    }
    button.classList.add("copied");
    button.setAttribute("aria-label", "Copied");
    button.innerHTML = CHECK_ICON;
    state.timeout = window.setTimeout(() => {
      button.classList.remove("copied");
      button.setAttribute("aria-label", state.label);
      button.innerHTML = state.html;
      state.timeout = null;
    }, 1400);
    copyButtonState.set(button, state);
  }

  function wireCopyButtons() {
    document.querySelectorAll("[data-copy-path], [data-copy-template]").forEach((button) => {
      button.addEventListener("click", async () => {
        const text = button.dataset.copyTemplate
          ? button.dataset.copyTemplate.replaceAll("{origin}", origin)
          : absoluteUrl(button.dataset.copyPath);
        try {
          await copyText(text, button);
        } catch (_error) {
          button.classList.add("copy-failed");
        }
      });
    });
  }

  function closeMenu(toggle, panel) {
    toggle.setAttribute("aria-expanded", "false");
    toggle.setAttribute("aria-label", "Open navigation menu");
    panel.hidden = true;
  }

  function openMenu(toggle, panel) {
    toggle.setAttribute("aria-expanded", "true");
    toggle.setAttribute("aria-label", "Close navigation menu");
    panel.hidden = false;
  }

  function wireMenus() {
    document.querySelectorAll("[data-menu-toggle]").forEach((toggle) => {
      const panelId = toggle.getAttribute("aria-controls");
      const panel = panelId ? document.getElementById(panelId) : null;
      if (!panel) {
        return;
      }

      toggle.addEventListener("click", () => {
        if (panel.hidden) {
          openMenu(toggle, panel);
        } else {
          closeMenu(toggle, panel);
        }
      });

      panel.querySelectorAll("a").forEach((link) => {
        link.addEventListener("click", () => closeMenu(toggle, panel));
      });

      document.addEventListener("click", (event) => {
        if (panel.hidden || toggle.contains(event.target) || panel.contains(event.target)) {
          return;
        }
        closeMenu(toggle, panel);
      });

      document.addEventListener("keydown", (event) => {
        if (event.key === "Escape" && !panel.hidden) {
          closeMenu(toggle, panel);
          toggle.focus();
        }
      });
    });
  }

  function wireCarousels() {
    document.querySelectorAll("[data-carousel]").forEach((carousel) => {
      const track = carousel.querySelector("[data-carousel-track]");
      const realSlides = Array.from(carousel.querySelectorAll(".carousel-slide"));
      const previous = carousel.querySelector("[data-carousel-prev]");
      const next = carousel.querySelector("[data-carousel-next]");
      const dotsHost = carousel.querySelector("[data-carousel-dots]");
      if (!track || !realSlides.length || !previous || !next || !dotsHost) {
        return;
      }

      const firstClone = realSlides[0].cloneNode(true);
      const lastClone = realSlides[realSlides.length - 1].cloneNode(true);
      firstClone.dataset.carouselClone = "true";
      lastClone.dataset.carouselClone = "true";
      firstClone.dataset.carouselTargetIndex = String(realSlides.length + 1);
      lastClone.dataset.carouselTargetIndex = "0";
      track.prepend(lastClone);
      track.append(firstClone);

      const slides = Array.from(track.querySelectorAll(".carousel-slide"));
      let index = 1;
      const dots = realSlides.map((slide, slideIndex) => {
        slide.dataset.carouselTargetIndex = String(slideIndex + 1);
        const dot = document.createElement("button");
        dot.className = "carousel-dot";
        dot.type = "button";
        dot.setAttribute("aria-label", `Show screenshot ${slideIndex + 1}`);
        dot.addEventListener("click", () => showSlide(slideIndex + 1));
        dotsHost.append(dot);
        slide.setAttribute("aria-label", `Screenshot ${slideIndex + 1} of ${realSlides.length}`);
        return dot;
      });

      function getDotIndex() {
        if (index === 0) {
          return realSlides.length - 1;
        }
        if (index === slides.length - 1) {
          return 0;
        }
        return index - 1;
      }

      function setIndex(nextIndex, options = {}) {
        index = nextIndex;
        track.classList.toggle("is-snapping", options.animate === false);
        carousel.style.setProperty("--carousel-index", index);
        slides.forEach((slide, slideIndex) => {
          const active = slideIndex === index;
          slide.classList.toggle("is-active", active);
          slide.setAttribute("aria-hidden", active ? "false" : "true");
          slide.querySelectorAll("a, button").forEach((element) => {
            element.tabIndex = active ? 0 : -1;
          });
        });
        const activeDotIndex = getDotIndex();
        dots.forEach((dot, dotIndex) => {
          if (dotIndex === activeDotIndex) {
            dot.setAttribute("aria-current", "true");
          } else {
            dot.removeAttribute("aria-current");
          }
        });
      }

      function showSlide(nextIndex) {
        setIndex(nextIndex, { animate: true });
      }

      previous.addEventListener("click", () => showSlide(index - 1));
      next.addEventListener("click", () => showSlide(index + 1));
      carousel.addEventListener("click", (event) => {
        const slide = event.target.closest(".carousel-slide");
        if (!slide || slide.classList.contains("is-active")) {
          return;
        }
        const targetIndex = Number(slide.dataset.carouselTargetIndex);
        if (Number.isFinite(targetIndex)) {
          event.preventDefault();
          showSlide(targetIndex);
        }
      });
      track.addEventListener("transitionend", () => {
        if (index === 0) {
          setIndex(realSlides.length, { animate: false });
        } else if (index === slides.length - 1) {
          setIndex(1, { animate: false });
        }
        requestAnimationFrame(() => track.classList.remove("is-snapping"));
      });
      carousel.addEventListener("keydown", (event) => {
        if (event.key === "ArrowLeft") {
          showSlide(index - 1);
        } else if (event.key === "ArrowRight") {
          showSlide(index + 1);
        }
      });

      setIndex(1, { animate: false });
      requestAnimationFrame(() => track.classList.remove("is-snapping"));
    });
  }

  fillOriginText();
  wireCopyButtons();
  wireMenus();
  wireCarousels();
})();
