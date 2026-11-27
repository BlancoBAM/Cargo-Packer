// background.js - Minimal MV3 service worker.
// The popup is fully self-contained and does not depend on this script;
// it exists so the manifest's service_worker entry resolves and the
// extension loads cleanly.

self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', (event) => event.waitUntil(self.clients.claim()));

// Keep the worker alive for basic message passing if ever needed.
chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  sendResponse({ success: true, received: message });
  return false;
});