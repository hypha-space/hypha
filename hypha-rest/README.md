# Hypha API Experiment

Experiment on structuring the API around resources and working with them through a RESTful API. This is similar to how Kubernetes works.
Workers would register by creating a `Worker` resource (not implemented here), then subscribe to a stream of tasks (implemented here).
A scheduler, knowing which workers exist (through `Worker` resources), creates tasks for them. Workers run these tasks and send task updates.
Different scheduling algorithm would be different resources as well, similar to operators in Kubernetes. With that, worker orchestration is done exclusively through `Task` resources, similar to pods in Kubernetes.

Furthermore, persisting state with [RocksDB](https://rocksdb.org/) is tested as well. RocksDB provides an in-process key-value-store that persists all state to disk.

## Example API Interactions

```
# Worker: Watch for task changes
curl 'http://localhost:8080/api/v0/tasks?watch=true'

# List tasks
curl 'http://localhost:8080/api/v0/tasks'

# Scheduler: Create a task
curl --header "Content-Type: application/json" \
     --request POST \
     --data '{"driver": "foo"}' \
     'http://localhost:8080/api/v0/tasks'

# Worker: Update a task
curl --header "Content-Type: application/json" \
     --request PUT \
     --data '{"id":"id","spec":{"driver":"foo"},"status":{"status":"running"}}' \
     'http://localhost:8080/api/v0/tasks/{id}'

# Delete a task
curl --request DELETE \
     'http://localhost:8080/api/v0/tasks/{id}'
```
