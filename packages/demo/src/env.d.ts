/** Build-time settings, read from the environment by Vite. */
interface ImportMetaEnv {
  /**
   * Where the Parquet page's sample recording is: any URL serving it with
   * CORS and range requests. Default `/sample.parquet`, where
   * `make_sample.py` writes it for local development.
   */
  readonly VITE_SAMPLE_URL?: string;
}
