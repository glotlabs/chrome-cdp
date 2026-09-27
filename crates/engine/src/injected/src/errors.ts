/** A mistake in the locator itself (a bad selector or regex): waiting or
 * retrying cannot help, so the engine reports it as `BadLocator`. */
export class LocatorError extends Error {}
