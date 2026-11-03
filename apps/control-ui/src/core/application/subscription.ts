/** Turns an asynchronously registered listener into one that can be removed at once. A
 * registration that completes after removal is undone immediately, so a listener never
 * outlives its owner; `onError` receives a registration that fails. */
export function removableSubscription(
  registration: Promise<() => void>,
  onError: (error: unknown) => void,
): () => void {
  let removed = false;
  let remove: (() => void) | undefined;
  registration.then(
    (unsubscribe) => {
      if (removed) unsubscribe();
      else remove = unsubscribe;
    },
    (error: unknown) => onError(error),
  );
  return () => {
    removed = true;
    remove?.();
  };
}
