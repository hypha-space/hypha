struct Location {
    path: String,
}
trait Router {
    async fn pull(&self, reference: &Reference) -> Result<Location, Error>;
    async fn push(&self, reference: &Reference, data: Location) -> Result<(), Error>;
}
