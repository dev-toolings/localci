<?php

declare(strict_types=1);

use App\Controller\GreetingController;
use App\Kernel;

require dirname(__DIR__) . '/vendor/autoload.php';

$controller = new GreetingController();

assertSame('Hello Ada from localci', $controller->greet('Ada'));
assertSame('Hello developer from localci', $controller->greet(''));

$kernel = new Kernel((string) ($_SERVER['APP_ENV'] ?? 'test'));
assertSame('test', $kernel->environment());

echo "All tests passed" . PHP_EOL;

function assertSame(string $expected, string $actual): void
{
    if ($expected !== $actual) {
        fwrite(STDERR, sprintf("Expected %s, got %s\n", $expected, $actual));
        exit(1);
    }
}
