// Main-world patch: every attachShadow call exposes its result via
// element.shadowRootUnl, letting us walk "closed" shadow roots. This is
// the same trick playwright-captcha uses to crawl CF's Turnstile DOM.
(() => {
  const originalAttachShadow = Element.prototype.attachShadow;
  Element.prototype.attachShadow = function (init) {
    const shadowRoot = originalAttachShadow.call(this, init);
    this.shadowRootUnl = shadowRoot;
    return shadowRoot;
  };
})();
