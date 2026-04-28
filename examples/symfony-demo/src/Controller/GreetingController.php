<?php

declare(strict_types=1);

namespace App\Controller;

final class GreetingController
{
    public function greet(string $name): string
    {
        $cleanName = trim($name);

        if ($cleanName === '') {
            $cleanName = 'developer';
        }

        return sprintf('Hello %s from localci', $cleanName);
    }
}
