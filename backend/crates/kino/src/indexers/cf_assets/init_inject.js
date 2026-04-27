// Content script: loads all patch scripts listed in scripts/registry.json
// and executes them in the MAIN world by injecting a <script> tag. Runs at
// document_start so patches are in place before page JS sees the shadow DOM.
async function loadScripts() {
  try {
    const registryResponse = await fetch(
      chrome.runtime.getURL("scripts/registry.json"),
    );
    const registry = await registryResponse.json();
    for (const scriptFile of registry) {
      const scriptResponse = await fetch(
        chrome.runtime.getURL(`scripts/${scriptFile}`),
      );
      const scriptContent = await scriptResponse.text();
      const script = document.createElement("script");
      script.textContent = scriptContent;
      document.documentElement.appendChild(script);
      script.remove();
    }
  } catch (_) {
    // swallow — best-effort
  }
}
loadScripts();
