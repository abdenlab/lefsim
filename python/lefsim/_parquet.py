"""Writing keyframed event streams to Parquet (requires pyarrow).

One row group per keyframed chunk: each row group opens with a keyframe and
covers the run until the next one, so the row groups' `time` statistics
index the file for random access.
"""

import json


def write_stream(stream, path, compression="zstd"):
    """Write a keyframed event `Stream` to `path`. See `Stream.to_parquet`."""
    import pyarrow as pa
    import pyarrow.parquet as pq

    schema = pa.schema(stream)
    run = json.loads(schema.metadata[b"lefsim.run"])
    # The layout marker goes in at the start, with the rest of the metadata,
    # so the file's stored notes and its schema's notes agree.
    # Sorted, so the same run always writes the same bytes.
    schema = schema.with_metadata(dict(sorted({**schema.metadata, b"lefsim.row_groups": b"keyframed"}.items())))
    # Event times are near-random doubles, most of the file; byte-stream-split
    # lays their bytes out so the compressor can shrink them (about a third
    # smaller files with zstd). Every Parquet reader supports it.
    # (Dictionary encoding takes precedence, so it's off for `time`.)
    dictionary = [name for name in schema.names if name != "time"]
    with pq.ParquetWriter(
        path, schema, compression=compression, use_dictionary=dictionary, use_byte_stream_split=["time"]
    ) as writer:
        try:
            for batch in stream:
                batch = pa.record_batch(batch).replace_schema_metadata(schema.metadata)
                if batch.num_rows:
                    writer.write_batch(batch, row_group_size=batch.num_rows)
        finally:
            # Where the data actually ends: the requested end if the stream
            # ran to completion, otherwise the end of the last batch written.
            run["record_end"] = stream.time
            writer.add_key_value_metadata({"lefsim.run": json.dumps(run)})
