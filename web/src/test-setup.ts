import "@testing-library/jest-dom/vitest";

// jsdom lays nothing out, so it has no scrolling.
Element.prototype.scrollIntoView = () => undefined;
