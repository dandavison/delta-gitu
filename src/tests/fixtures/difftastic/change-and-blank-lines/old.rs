fn run(job: Job) {
    job.prepare();
    job.execute(1);
    job.report();
}
