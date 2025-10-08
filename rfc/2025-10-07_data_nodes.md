# Data Node Specification

## Overview

To improve data distribution in distributed training environments, we introduce data nodes, which are dedicated servers responsible for managing and distributing data sets to worker nodes.

## Background

For machine learning tasks, whether it's foundational training, fine-tuning, or inference, data sets are used as input.
While a plethora of data sets exist, they often have to be prepared for training, e.g. converted to a suitable data format, quantized, or tokenized. Futhermore, even after preparation, these data sets can be very large. When doing data-parallel training, not all of the data is needed on all worker nodes at the time.
With these challenges in mind, we propose a solution that addresses the need for efficient data distribution and management in distributed training environments.

## Proposal

Data nodes provide data to workers from a prepared data set. We will provide tools for users to prepare existing data sets into a format suitable for data nodes. Users are also responsible to batch data sets into multiple files suitable for data-parallel training.
A data node announces its available data set using the Kademlia DHT. Schedulers, when instructed to train on a data set, will query the DHT for it and send the peer ID of the data node providing that data set to workers together with information on what batches of the dataset to use. Workers then pull the respective dataset batches from the data node using point-to-point communication.
Data set batches are provided as SafeTensor files. While SafeTensor is primarily aimed at representing model parameters, it can also work well for data that has been prepared, e.g. already tokenized.

## Abandoned Ideas

### Store prepared data sets on HuggingFace

It's possible to store prepared data sets on HuggingFace, but this requires additional infrastructure and maintenance effort. Additionally, HuggingFace is not well suited for efficient distribution of data set batches as needed for data-parallel training.
