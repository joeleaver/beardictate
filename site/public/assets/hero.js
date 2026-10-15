// The landing page's one moment of motion: the spoken words appear as if heard, then
// the cleaned sentences land on the page. Skipped entirely under reduced motion.
(function () {
  var scene = document.getElementById("scene");
  if (!scene) return;
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    scene.classList.add("done");
    return;
  }
  var words = scene.querySelectorAll(".said .w");
  var sentences = scene.querySelectorAll(".page .s");
  // Each sentence lands once the phrase that ends it has been heard.
  var landAfter = [5, 7, 10];
  var t = 400;
  words.forEach(function (w, i) {
    t += 260 + w.textContent.length * 18;
    setTimeout(function () {
      w.classList.add("on");
      landAfter.forEach(function (at, s) {
        if (at === i) setTimeout(function () { sentences[s].classList.add("on"); }, 500);
      });
    }, t);
  });
  setTimeout(function () { scene.classList.add("done"); }, t + 1400);
})();
