export const memoryStorage = (seed = {}) => {
  const entries = new Map(Object.entries(seed));
  return {
    getItem: (key) => (entries.has(key) ? entries.get(key) : null),
    setItem: (key, value) => entries.set(key, String(value)),
    removeItem: (key) => entries.delete(key),
    entries,
  };
};

export const refusingStorage = () => ({
  getItem: () => {
    throw new Error("private mode");
  },
  setItem: () => {
    throw new Error("private mode");
  },
  removeItem: () => {
    throw new Error("private mode");
  },
});

export const writeRefusingStorage = (seed = {}) => ({
  ...memoryStorage(seed),
  setItem: () => {
    throw new Error("private mode");
  },
});
