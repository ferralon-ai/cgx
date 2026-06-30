package com.example.fx;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Instant;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.locks.Lock;

// Fixture for Cycle 4 own-effects + concurrency/async detection.
class Effects {

    // io.file: console stream write + NIO read.
    void writeReport(Path p) throws IOException {
        System.out.println("start");
        byte[] data = Files.readAllBytes(p);
        System.err.println(data.length);
    }

    // pure: arithmetic only, no effect.
    int add(int a, int b) {
        return a + b;
    }

    // throwing only: no GM-12 effect analogue (documented gap).
    void mustBePositive(int n) {
        if (n <= 0) {
            throw new IllegalArgumentException("non-positive");
        }
    }

    // blocking: synchronized method holds the monitor for its whole body.
    synchronized void increment() {
        this.counter++;
    }

    // blocking: synchronized block + explicit lock acquisition.
    void guarded(Lock lock) {
        synchronized (this) {
            this.counter++;
        }
        lock.lock();
    }

    // spawns: thread creation + start.
    void launch(Runnable r) {
        Thread t = new Thread(r);
        t.start();
    }

    // spawns: executor submit.
    void offload(ExecutorService pool, Runnable r) {
        pool.submit(r);
    }

    // nondeterministic: clock + randomness.
    long stamp() {
        long now = Instant.now().toEpochMilli();
        return now + (long) (Math.random() * 10);
    }

    private int counter;
}
