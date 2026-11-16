package com.acme.app;

import com.acme.app.util.Helper;
import java.util.List;

public class Sample extends Base implements Runner {
    public Sample() {}

    @Override
    public void run() {
        Helper.assist();
        step();
        new Other().go();
    }

    private void step() {}
}

interface Runner extends Marker { void run(); }
