const bridgeLifetime = new Promise(() => {
  document.addEventListener("click", (event) => {
    if (!(event instanceof MouseEvent)) return;

    const { clientX: x, clientY: y } = event;
    const insideViewport =
      Number.isFinite(x) &&
      Number.isFinite(y) &&
      x >= 0 &&
      y >= 0 &&
      x <= window.innerWidth &&
      y <= window.innerHeight;

    if (insideViewport) dioxus.send({ type: "tap", x, y });
  });
});

// Dioxus closes dioxus.send when an eval script resolves.
await bridgeLifetime;
