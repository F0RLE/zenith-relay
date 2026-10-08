/** Keep only the newest asynchronous result without cancelling in-flight work. */
export class LatestRequestGate {
  private revision = 0;

  invalidate() {
    this.revision += 1;
  }

  async run<T>(load: () => Promise<T>, commit: (loadedValue: T) => void): Promise<T> {
    const requestRevision = ++this.revision;
    const loadedValue = await load();
    if (requestRevision === this.revision) commit(loadedValue);
    return loadedValue;
  }
}
