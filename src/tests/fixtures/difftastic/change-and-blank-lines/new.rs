fn run(job: Job) {
    job.prepare();

    job.execute(2);

    job.report();
}
