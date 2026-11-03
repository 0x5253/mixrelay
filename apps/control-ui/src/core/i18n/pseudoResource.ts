type TranslationResource = { readonly [key: string]: string | TranslationResource };
type PseudoResource<T> = {
  readonly [Key in keyof T]: T[Key] extends string ? string : PseudoResource<T[Key]>;
};

/** Fill every source key; overrides keep deliberate accents and longer review cases. */
export function pseudoResource<T extends TranslationResource>(
  source: T,
  overrides: TranslationResource = {},
): PseudoResource<T> {
  return Object.fromEntries(
    Object.entries(source).map(([key, value]) => {
      const override = overrides[key];
      return [
        key,
        typeof value === 'string'
          ? typeof override === 'string'
            ? override
            : `[!! ${value} — expanded translation !!]`
          : pseudoResource(value, typeof override === 'object' ? override : {}),
      ];
    }),
  ) as PseudoResource<T>;
}
