"""Writing keyframed event streams to Parquet (requires pyarrow).

One row group per keyframed chunk: each row group opens with a keyframe and
covers the run until the next one, so the row groups' `time` statistics
index the file for random access.
"""

import json


def write_stream(stream, path):
    """Write a keyframed event `Stream` to `path`. See `Stream.to_parquet`."""
    import pyarrow as pa
    import pyarrow.parquet as pq

    schema = pa.schema(stream)
    run = json.loads(schema.metadata[b"lefsim.run"])
    with pq.ParquetWriter(path, schema) as writer:
        try:
            for batch in stream:
                batch = pa.record_batch(batch)
                if batch.num_rows:
                    writer.write_batch(batch, row_group_size=batch.num_rows)
        finally:
            # Where the data actually ends: the requested end if the stream
            # ran to completion, otherwise the end of the last batch written.
            run["record_end"] = stream.time
            writer.add_key_value_metadata({"lefsim.run": json.dumps(run), "lefsim.row_groups": "keyframed"})
