declare module "*.css";

declare function acquireVsCodeApi(): {
  postMessage(message: unknown): void;
};
