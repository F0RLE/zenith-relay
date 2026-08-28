/** Keep only the newest asynchronous result without cancelling in-flight work. */
export class LatestRequestGate {
  private revision = 0;

  invalidate() {
    this.revision += 1;
  }

  async run<T>(load: () => Promise<T>, commit: (value: T) => void): Promise<T> {
    const request = ++this.revision;
    const value = await load();
    if (request === this.revision) commit(value);
    return value;
  }
}
