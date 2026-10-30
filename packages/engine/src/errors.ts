export type EngineErrorCode =
  | 'invalid_id'
  | 'invalid_input'
  | 'invalid_config'
  | 'invalid_state'
  | 'not_found'
  | 'missing_secret'
  | 'no_verified'
  | 'orphans'
  | 'teardown_incomplete'
  | 'unavailable'
  | 'aborted'
  | 'sandbox_not_applied'
  | 'sandbox_in_use'
  | 'not_configured';

export class EngineError extends Error {
  constructor(
    readonly code: EngineErrorCode,
    message: string,
    readonly details?: unknown,
  ) {
    super(message);
    this.name = 'EngineError';
  }
}

const ID_RE = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
const MAX_ID_LEN = 48;

/** Ids end up in branch names, IAM role names and SSM paths: lowercase letters, digits and single dashes only. */
export function assertValidId(what: string, id: unknown): asserts id is string {
  if (typeof id !== 'string' || id.length === 0 || id.length > MAX_ID_LEN || !ID_RE.test(id)) {
    throw new EngineError(
      'invalid_id',
      `invalid ${what} ${JSON.stringify(id)}: use 1-${MAX_ID_LEN} characters from [a-z0-9-], no leading, trailing or doubled dashes`,
    );
  }
}
