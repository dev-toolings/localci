<?php

declare(strict_types=1);

use App\Controller\GreetingController;

require dirname(__DIR__) . '/vendor/autoload.php';

header('Content-Type: text/plain; charset=utf-8');

$name = $_GET['name'] ?? 'developer';

echo (new GreetingController())->greet((string) $name);
