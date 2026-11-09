(function () {
  var root = document.documentElement;
  if (!root || root.dataset.folio !== "chrome") {
    return;
  }
  document.addEventListener("click", function (event) {
    if (typeof dioxus === "undefined" || typeof dioxus.send !== "function") {
      return;
    }
    dioxus.send(
      JSON.stringify({
        kind: "tap",
        x: event.clientX,
        y: event.clientY,
      })
    );
  });
})();
