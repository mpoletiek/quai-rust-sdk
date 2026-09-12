// TypeScript prints absolute module names for namespace exports. Normalize only
// the package root, keeping declarations and symbol text otherwise unchanged.
export function portableInventory(value, packageRoot) {
  if (typeof value === 'string') return value.split(packageRoot.replaceAll('\\', '/').replace(/\/$/, '')).join('quais');
  if (Array.isArray(value)) return value.map(item => portableInventory(item, packageRoot));
  if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, portableInventory(item, packageRoot)]));
  return value;
}
