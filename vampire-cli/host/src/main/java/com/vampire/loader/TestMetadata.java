package com.vampire.loader;

public class TestMetadata {
    private final String name;
    private final boolean isAsync;
    private final boolean shouldPanic;

    public TestMetadata(String name, boolean isAsync, boolean shouldPanic) {
        this.name = name;
        this.isAsync = isAsync;
        this.shouldPanic = shouldPanic;
    }

    public String getName() {
        return name;
    }

    public boolean isAsync() {
        return isAsync;
    }

    public boolean shouldPanic() {
        return shouldPanic;
    }
}
