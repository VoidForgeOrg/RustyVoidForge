import "@testing-library/jest-dom/vitest";

// jsdom lays nothing out, so it has no scrolling and nothing to observe resizing.
Element.prototype.scrollIntoView = () => undefined;
globalThis.ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
