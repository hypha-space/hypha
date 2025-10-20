# Performance-Aware Scheduling

## Overview

With heterogenous hardware, Composite Learning (CL) faces a significant challenge in data scheduling.
Traditionally, all compute devices run with the same configuration, since they are expected to provide
the same memory and FLOPS. However, with CL this paradigm changes, and each participant in a training
needs to run on a different batch size and batches until DiLoCo synchronisation.

## Background

To handle the above challenge, this RFC introduces a two-fold scheduling for the data. The first phase handles 
data distribution to the Worker, s.t. a Worker will always have data available to train on.
The second phase is the crucial part for optimizing performance. It determines how many batches each Worker
should process until the next DiLoCo update. Thus, the Scheduler determines from the pre-defined number
of `outer-steps` and a theoretical `batch_size`, how many batches each Worker should process until
the number of `outer_steps` is completed. 

## Proposal

This RFC proposes the following structure, where data serving and training are split into two different processes.

```Mermaid
---
config:
  theme: redux
---
sequenceDiagram
    participant S as Scheduler
    participant D as Data 
    participant W as Worker
    participant PS as Parameter Server
    loop Data Loading    
        W-->> S: Request Data
        S -->> W: Slice ID
        alt Slice cached 
            W -->> W: load from cache
        else 
             W -->> D: Request Slice
        end
        D -->> W: Stream Slice
        alt Slice contains more than x batches 
        W -->> W: Yield batches from Slice
        else Prefetch
        
        end
        W -->> S: Slice ID done
    end
    loop Training loop
        W -->> W: load batch, forward + backward
        W -->> S: metric + num. data points
        alt Work
            S -->> W: ACK
        else is Schedule Update
            S -->> W: Schedule Update + counter
            S -->> PS: Schedule Update num. worker ?+ timout?
            W -->> W: Initializer counter
        else is Update
            W -->> PS: Send Weights
            PS -->> PS: Nesterov
            PS -->> W: Send Weights
        end
    end
```

The Data Loading is an independent process on the Scheduler, where Worker request data slices. 
A Worker sends a request for data. The Scheduler determines which data slice is available for training and
sends it to the Worker. The Worker then requests the slice from a Data Node and yields batches from it to
the training process. To optimize for waiting times, the Worker should be able to perform prefetching to always
have local training data available.

The Scheduler needs to track the states of the data slices. It can be in any of the three states.
```Rust
struct enum State{
    AVAILABLE,
    ASSIGNED,
    USED
}
```
Furthermore, the Scheduler also needs to reassign slices to Workers after all slices have been used once.

The Training loop part of the scheduling enables the Scheduler to control the training flow. Once a Worker performed
a single training step (data loading into memory, forward and backward step), it will send metrics (loss, etc.) and the 
`batch_size` to the Scheduler. The Scheduler will then update how many data points need to be processed before
triggering the next DiLoCo update. To determine when the next update should be scheduled, the Scheduler also tracks the
average time a worker needs to process a batch. With this information in place, it can simulate into the 
futuer how many batches each worker will complete until an update. 

The simulation can be something like this:
```Python
def project(to_go: int, response_times: np.array, batch_sizes: np.array, time_steps: int = 10, time_cap: int = 5 * 60 * 1000):
  updates = np.zeros(batch_sizes.shape)
  next_update = deepcopy(response_times)
  time = 0
  done = 0
  while np.max(updates) < time_steps and time < time_cap and (to_go - done) > 0:
    update_idx = np.argmin(next_update)
    time = next_update[update_idx]
    done += batch_sizes[update_idx]
    updates[update_idx] += 1
    next_update[update_idx] += response_times[update_idx]
  return time, done, (to_go - done) , updates
```

Once the simulation determines that an update should happen within the next few local rounds of a worker (e.g., 
the slowest worker will only need to process 5 more batches), the Scheduler schedules a DiLoCo update by 
informing the Worker how many batches to process until sending the pseudo-gradient to the Parameter Server.
The Worker will start a counter until it's time to send the update.

The Scheduler will also trigger an update on the Parameter Server by sending the PeerIds from the Workers
that will send an update and a timout, until which the Parater Server will wait for Workers to initiate
the send process of the updates.

If the Scheduler determines that no update needs to be scheduled, it will just acknowledge the Workers message.

## Abandoned Ideas

This RFC clearly separates data fetching and training on the Scheduler and Worker. This is mostly because
the data serving and training are two different parts in Worker. Within the training loop, the Worker
only requests a batch from a `DataLoader`. Thus, no information on the specific underlying data slice
is required. However, the Worker needs to know if it should continue training or perform an update.

On the other side. The `DataLoader` doesn't know if the Worker is training or updating the weights. It
only needs to know which data slice it should provide to the training process. Therefore, to simplify the
logic on Worker and Scheduler the two processes run independently.