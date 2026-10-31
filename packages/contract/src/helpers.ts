// Type-level test helpers. Nothing here exists at runtime.

/** Deeply flattens intersections and aliases so structurally identical types compare equal. */
export type Norm<T> = T extends readonly (infer U)[]
  ? Norm<U>[]
  : T extends (...args: never[]) => unknown
    ? T
    : T extends object
      ? { [K in keyof T]: Norm<T[K]> }
      : T;

type Identical<A, B> = (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2 ? true : false;

/** Exact equality after normalisation: same keys, same optionality, same value types. */
export type Equals<A, B> = Identical<Norm<A>, Norm<B>>;

/** `true` when equal; otherwise an object type carrying both sides, so the compiler error shows what differs. */
export type Same<A, B> = Equals<A, B> extends true ? true : { mismatch: { ts: Norm<A>; wire: Norm<B> } };

/** Every value of `A` is a value of `B` (one direction only). */
export type Within<A, B> = [A] extends [B] ? true : { notAssignable: { from: Norm<A>; to: Norm<B> } };

/**
 * For request bodies: what the TypeScript client sends must be accepted by the contract (`Within`) and must not
 * leave a contract field unnamed: the keys of the contract that the TypeScript type does not have must be exactly
 * `Extra` (fields of features that are not in the desktop app, or that the daemon accepts as an extension).
 */
export type Sends<Ts extends object, Wire extends object, Extra extends PropertyKey = never> =
  Within<Ts, Wire> extends true
    ? [Exclude<keyof Wire, keyof Ts>] extends [Extra]
      ? [Extra] extends [Exclude<keyof Wire, keyof Ts>]
        ? [Exclude<keyof Ts, keyof Wire>] extends [never]
          ? true
          : { tsOnlyKeys: Exclude<keyof Ts, keyof Wire> }
        : { extraNotInWire: Exclude<Extra, Exclude<keyof Wire, keyof Ts>> }
      : { wireOnlyKeys: Exclude<Exclude<keyof Wire, keyof Ts>, Extra> }
    : Within<Ts, Wire>;

/** The rows of a table that are not `true`. */
export type Failing<T> = { [K in keyof T as T[K] extends true ? never : K]: T[K] };

/** `true` when every row of the table is `true`; otherwise the failing rows, by name, with both sides. */
export type AssertAll<T> = [keyof Failing<T>] extends [never] ? true : { failingRows: keyof Failing<T>; details: Failing<T> };

/** Compile error unless `T` is `true`: `export type Rows = Expect<AssertAll<{...}>>`. */
export type Expect<T extends true> = T;

/** Replace the named properties of `T` with those of `Patch` (an intentional, documented difference). */
export type Patch<T, P> = Omit<T, keyof P> & P;

/** The members of a tagged union, by tag. */
export type ByType<U extends { type: string }, K extends U['type']> = Extract<U, { type: K }>;
