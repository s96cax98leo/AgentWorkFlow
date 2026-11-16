package com.acme.app

import com.acme.app.util.Helper
import com.acme.app.util.assist as a

interface Runner { fun run() }

class Sample(val name: String) : Base(), Runner {
    override fun run() {
        Helper.assist()
        step()
        a(name)
    }

    private fun step() {}
}

object Registry { fun get(): Sample = Sample("x") }

fun main() {
    Registry.get().run()
}
