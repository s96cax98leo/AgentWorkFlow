using System;
using Acme.App.Util;

namespace Acme.App
{
    public interface IRunner { void Run(); }

    public class Sample : Base, IRunner
    {
        public Sample() {}

        public void Run()
        {
            Helper.Assist();
            Step();
            var o = new Other();
        }

        private void Step() {}
    }
}
