/**
 * Utility functions for consistent error handling across the application.
 */

export type BackendErrorParam = string | number | boolean;

export interface BackendError {
  version: 1;
  code: string;
  messageKey: string;
  messageParams: Record<string, BackendErrorParam>;
  source: "jdbcAgent" | "jdbcAgentLegacy" | "legacyBackend";
  operationOutcome: "not_started" | "unknown";
  detail?: string;
  diagnostics?: Record<string, unknown>;
  helpUrl?: string;
}

const MAX_FALLBACK_CHARS = 512;

function isBackendError(value: unknown): value is BackendError {
  if (!value || typeof value !== "object") return false;
  const candidate = value as Record<string, unknown>;
  if (
    candidate.version !== 1 ||
    typeof candidate.code !== "string" ||
    !/^DBX-[A-Z][A-Z0-9]*-\d{4}$/.test(candidate.code) ||
    typeof candidate.messageKey !== "string" ||
    !candidate.messageKey.startsWith("backendErrors.") ||
    !candidate.messageParams ||
    typeof candidate.messageParams !== "object" ||
    Array.isArray(candidate.messageParams) ||
    !["jdbcAgent", "jdbcAgentLegacy", "legacyBackend"].includes(String(candidate.source)) ||
    !["not_started", "unknown"].includes(String(candidate.operationOutcome))
  ) {
    return false;
  }
  if (candidate.detail !== undefined && typeof candidate.detail !== "string") return false;
  return Object.values(candidate.messageParams).every((param) => typeof param === "string" || typeof param === "boolean" || (typeof param === "number" && Number.isFinite(param)));
}

export function normalizeBackendError(error: unknown): BackendError | null {
  if (error instanceof BackendErrorException) return error.backendError;
  if (typeof error === "string") {
    try {
      return normalizeBackendError(JSON.parse(error));
    } catch {
      return null;
    }
  }
  if (error instanceof Error) {
    try {
      const parsed: unknown = JSON.parse(error.message);
      return normalizeBackendError(parsed);
    } catch {
      return null;
    }
  }
  if (isBackendError(error)) return error;
  if (error && typeof error === "object" && "backendError" in error) {
    const backendError = (error as { backendError: unknown }).backendError;
    const normalized = normalizeBackendError(backendError);
    if (normalized) return normalized;
  }
  if (error && typeof error === "object" && "error" in error) {
    const nested = (error as { error: unknown }).error;
    const normalized = normalizeBackendError(nested);
    if (normalized) return normalized;
  }
  return null;
}

export class BackendErrorException extends Error {
  readonly backendError: BackendError;

  constructor(error: unknown) {
    const backendError = normalizeRawBackendError(error);
    const fallbackDetail = boundedFallbackText(error);
    const fallbackMessage = fallbackDetail ?? "Backend request failed";
    super(backendError?.detail || fallbackMessage);
    this.name = "BackendErrorException";
    this.backendError = backendError ?? {
      version: 1,
      code: "DBX-LEGACY-0001",
      messageKey: "backendErrors.legacy",
      messageParams: {},
      source: "legacyBackend",
      operationOutcome: "unknown",
      ...(fallbackDetail ? { detail: fallbackDetail } : {}),
    };
  }
}

function normalizeRawBackendError(error: unknown): BackendError | null {
  return normalizeBackendError(error);
}

function boundedFallbackText(error: unknown): string | undefined {
  let text: string | undefined;
  if (typeof error === "string") {
    text = error;
  } else if (error instanceof Error) {
    text = error.message;
  } else if (error && typeof error === "object") {
    const candidate = error as Record<string, unknown>;
    for (const key of ["message", "reason", "detail"]) {
      if (typeof candidate[key] === "string") {
        text = candidate[key];
        break;
      }
    }
    if (!text && "error" in candidate) text = boundedFallbackText(candidate.error);
  }
  const normalized = text?.trim();
  if (!normalized) return undefined;
  return Array.from(normalized).slice(0, MAX_FALLBACK_CHARS).join("");
}

/**
 * Formats an unknown error value into a human-readable string.
 * Handles Error objects, strings, null/undefined, and other types.
 *
 * @param e - The error value to format (from a catch block)
 * @returns A human-readable error message string
 *
 * @example
 * try {
 *   await someOperation();
 * } catch (e: unknown) {
 *   errorMessage.value = formatError(e);
 * }
 */
export function formatError(e: unknown): string {
  const backendError = normalizeBackendError(e);
  if (backendError?.detail) return backendError.detail;
  if (backendError) return backendError.code;

  if (e instanceof Error) {
    return e.message;
  }

  if (typeof e === "string") {
    return e;
  }

  if (e === null || e === undefined) {
    return "Unknown error occurred";
  }

  // Try to extract message property from object-like values
  if (typeof e === "object" && "message" in e) {
    const message = (e as { message: unknown }).message;
    if (typeof message === "string") {
      return message;
    }
  }

  // Fallback: attempt to stringify
  try {
    return String(e);
  } catch {
    return "Unknown error occurred";
  }
}

/**
 * Formats an error with a context prefix for better debugging.
 *
 * @param e - The error value to format
 * @param context - The operation context (e.g., "loading topics", "creating tenant")
 * @returns A formatted error message with context
 *
 * @example
 * catch (e: unknown) {
 *   errorMessage.value = formatErrorWithContext(e, 'loading topics');
 * }
 */
export function formatErrorWithContext(e: unknown, context: string): string {
  const message = formatError(e);
  return `Failed to ${context}: ${message}`;
}
