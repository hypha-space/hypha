+++
title = "Troubleshooting"
description = "Aid in resolving common issues encountered when using Hypha."
weight = 30
[taxonomies]
track = ["reference"]
+++

# Troubleshooting

This guide addresses common issues encountered when using Hypha. Follow the steps for each error to resolve them effectively.

For better navigation, the guide is structured by component.

## Gateway

## Worker

### Snappy Corrupt Input

When starting a training and a Worker fails immediately with the following message:

```LOG
2025-11-26T09:38:35.115470Z  INFO hypha_worker::executor::bridge: Copied resource size=131 file=/tmp/hypha-df71010f-4977-41d2-a694-b4ffde3a590b/artifacts/0
Traceback (most recent call last):
  File "/Users/test/.cache/uv/archive-v0/wu732xbcarPOjBnemLBfq/lib/python3.12/site-packages/snappy/snappy.py", line 84, in uncompress
    out = bytes(_uncompress(data))
                ^^^^^^^^^^^^^^^^^
cramjam.DecompressionError: snappy: corrupt input (expected valid offset but got offset 882; dst position: 0)
```



#### Solution A

Make sure that the dataset was successfuly downloaded. A `size=131` is an indicator that the cloning a dataset from a repositry only cloned the `LFS pointer` and not the actuall files. To verify this use `cat <file_in_repo>`. If the content is similar to this:

```
version https://git-lfs.github.com/spec/v1
oid sha256:63ecc7d8cd62fef16d3d6c50463644fa81dd9d92506bc9ca7a8f4fdb58579d8a
size 14851
```

Then remove the repository, ensure that `git lfs`  is installed and clone the dataset again.

#### Solution B

If the `size` indicated in the `hypha_worker::executor::bridge` message, matches the size of the file in the dataset, make sure that all files in the dataset are compressed with [Snappy](https://github.com/google/snappy).

## Data

## Cert-Util

## AIM
