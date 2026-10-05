package com.example.dispatch;

public class VirtualDispatch {
    public static String makeChirp(Chirper c) {
        return c.chirp();
    }
}

interface Chirper {
    String chirp();
}

class Dog implements Chirper {
    @Override
    public String chirp() {
        return "woof";
    }
}

class Cat implements Chirper {
    @Override
    public String chirp() {
        return "meow";
    }
}
