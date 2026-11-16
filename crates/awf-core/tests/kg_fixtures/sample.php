<?php
namespace Acme\App;

use Acme\App\Util\Helper;
use Acme\App\Contracts\Runner as R;
require_once 'bootstrap.php';

class Sample extends Base implements R
{
    public function run(): void
    {
        Helper::assist();
        $this->step();
        $o = new Other();
        format_name("x");
    }

    private function step(): void {}
}

function format_name(string $n): string { return trim($n); }
